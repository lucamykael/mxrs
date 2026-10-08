//! A published REST service is declared once: the build writes the model's
//! service from the declaration, and the router serves exactly what it
//! declares.

use axum::Router;
use axum::body::Body;
use axum::extract::Path;
use axum::http::{Request, StatusCode};
use mxrs::prelude::*;
use mxrs::rest::Service;
use tower::ServiceExt;

async fn index() -> &'static str {
    "every order"
}

async fn show(Path(id): Path<String>) -> String {
    format!("order {id}")
}

async fn create() -> StatusCode {
    StatusCode::CREATED
}

pub fn service() -> Service<()> {
    Service::new("OrdersApi", "api/v1")
        .service_name("Orders API")
        .basic_authentication(&["Sales.User"])
        .resource("orders", |orders| {
            orders
                .get("", "Sales.MF_Order_List", index, |_| {})
                .post("", "Sales.MF_Order_Create", create, |operation| {
                    operation.query_parameter("note", RestParameterType::String);
                })
                .get("{id}", "Sales.MF_Order_Show", show, |operation| {
                    operation
                        .summary("One order")
                        .path_parameter("id", RestParameterType::Integer)
                        .export_mapping("Sales.EM_Order");
                });
        })
}

/// The microflows the operations call.
#[declaration(module = "Sales")]
pub fn order_flows(module: &mut ModuleBuilder) {
    module
        .microflow("MF_Order_List", |_| {})
        .microflow("MF_Order_Create", |flow| {
            flow.parameter::<MxString>("note", |_| {});
        })
        .microflow("MF_Order_Show", |flow| {
            flow.parameter::<MxLong>("id", |_| {});
        });
}

#[declaration(module = "Sales")]
pub fn orders_api(module: &mut ModuleBuilder) {
    module.published_rest_service(service().declaration());
}

#[mxrs::application(version = "11.12.1")]
pub struct Application;

async fn answer(router: &Router, method: &str, uri: &str) -> (StatusCode, String) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

/// Each operation answers its method at its service's path, its resource's
/// name and its own path; nothing else is routed.
#[tokio::test]
async fn the_router_serves_what_the_service_declares() {
    let router = service().router();
    assert_eq!(
        answer(&router, "GET", "/api/v1/orders").await,
        (StatusCode::OK, "every order".to_string())
    );
    assert_eq!(
        answer(&router, "GET", "/api/v1/orders/7").await,
        (StatusCode::OK, "order 7".to_string())
    );
    assert_eq!(
        answer(&router, "POST", "/api/v1/orders").await.0,
        StatusCode::CREATED
    );
    assert_eq!(
        answer(&router, "DELETE", "/api/v1/orders/7").await.0,
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(
        answer(&router, "GET", "/orders").await.0,
        StatusCode::NOT_FOUND
    );
}

/// The declaration registers itself, and the build writes the service as
/// Studio Pro stores one: it reads back as the declaration.
#[test]
fn the_build_writes_the_service_it_declares() {
    let project = Application::build();
    let sales = project
        .modules
        .iter()
        .find(|module| module.name == "Sales")
        .expect("the Sales module is declared");
    assert_eq!(sales.published_rest_services, [service().declaration()]);

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("App.mpr");
    mxrs::write_project(&path, &project).unwrap();
    let model = mxrs_model::Project::open(&path, true).unwrap();
    let stored: Vec<_> = model
        .all_units()
        .unwrap()
        .iter()
        .filter_map(|unit| model.mpr().parse_contents(unit).ok())
        .filter(|document| document.get_str("$Type").ok() == Some("Rest$PublishedRestService"))
        .collect();
    assert_eq!(stored.len(), 1);
    let stated = mxrs_writer::stated_document(&stored[0]).unwrap();
    assert_eq!(
        mxrs::PublishedRestServiceDecl::from_document(&stated),
        Some(service().declaration())
    );
}
