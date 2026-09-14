use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::response::Response;
use http_body_util::BodyExt as _;
use mxrs_runtime::{Runtime, RuntimeError, SecurityContext, SecurityPolicy, Store, StoreSchema};
use mxrs_runtime_http::{HttpError, RuntimeHttp};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tower::ServiceExt as _;

fn web_root() -> tempfile::TempDir {
    let web = tempfile::tempdir().unwrap();
    std::fs::write(web.path().join("index.html"), "<h1>MXRS</h1>").unwrap();
    web
}

fn runtime() -> Runtime {
    let mut runtime = Runtime::new(
        Store::new(StoreSchema::default()),
        SecurityPolicy::default(),
    );
    runtime.register_action("Sales.Echo", |_store: &mut Store, value: &Value| {
        Ok(value.clone())
    });
    runtime
}

fn action_request(path: &str) -> Request<Body> {
    Request::post(path)
        .header("content-type", "application/json")
        .body(Body::from(r#"{"value":42}"#))
        .unwrap()
}

async fn json_body(response: Response) -> Value {
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

#[tokio::test]
async fn only_supported_action_methods_and_json_bodies_reach_the_runtime() {
    let web = web_root();
    let router = RuntimeHttp::new(runtime(), web.path()).router().unwrap();
    for kind in ["action", "microflow", "nanoflow"] {
        let response = router
            .clone()
            .oneshot(action_request(&format!("/api/{kind}/Sales.Echo")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(json_body(response).await, json!({"result":{"value":42}}));
    }
    for method in [
        Method::GET,
        Method::HEAD,
        Method::OPTIONS,
        Method::PUT,
        Method::DELETE,
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri("/api/action/Sales.Echo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert!(
            !response
                .headers()
                .contains_key("access-control-allow-origin")
        );
    }
    for (content_type, body, status) in [
        (
            "text/plain",
            "{}".to_string(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (
            "application/x-www-form-urlencoded",
            "value=42".to_string(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        ("application/json", "{".to_string(), StatusCode::BAD_REQUEST),
        (
            "application/json",
            format!("\"{}\"", "x".repeat(1024 * 1024)),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::post("/api/action/Sales.Echo")
                    .header("content-type", content_type)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
    let response = router
        .oneshot(action_request("/api/unsupported/Sales.Echo"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_body(response).await["error"], "unknown_action_kind");
}

#[tokio::test]
async fn browser_actions_require_same_origin_and_do_not_trust_forwarded_headers() {
    let web = web_root();
    let router = RuntimeHttp::new(runtime(), web.path()).router().unwrap();
    for (origin, site, status) in [
        ("http://localhost:8080", "same-origin", StatusCode::OK),
        ("http://attacker.test", "cross-site", StatusCode::FORBIDDEN),
        (
            "http://other.localhost:8080",
            "same-site",
            StatusCode::FORBIDDEN,
        ),
        ("null", "same-origin", StatusCode::FORBIDDEN),
        (
            "https://localhost:8080",
            "same-origin",
            StatusCode::FORBIDDEN,
        ),
    ] {
        let mut request = action_request("/api/action/Sales.Echo");
        request
            .headers_mut()
            .insert("host", "localhost:8080".parse().unwrap());
        request
            .headers_mut()
            .insert("origin", origin.parse().unwrap());
        request
            .headers_mut()
            .insert("sec-fetch-site", site.parse().unwrap());
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), status, "{origin}");
        if status == StatusCode::FORBIDDEN {
            assert_eq!(json_body(response).await["error"], "cross_origin_request");
        }
    }
    let proxy = RuntimeHttp::new(runtime(), web.path())
        .with_public_origin("https://app.example.com")
        .unwrap()
        .router()
        .unwrap();
    let mut rebound = action_request("/api/action/Sales.Echo");
    rebound
        .headers_mut()
        .insert("host", "attacker.example:8080".parse().unwrap());
    rebound
        .headers_mut()
        .insert("origin", "http://attacker.example:8080".parse().unwrap());
    rebound
        .headers_mut()
        .insert("sec-fetch-site", "same-origin".parse().unwrap());
    assert_eq!(
        router.oneshot(rebound).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    for (origin, status) in [
        ("https://app.example.com", StatusCode::OK),
        ("https://attacker.test", StatusCode::FORBIDDEN),
    ] {
        let mut request = action_request("/api/action/Sales.Echo");
        request
            .headers_mut()
            .insert("host", "127.0.0.1:8080".parse().unwrap());
        request
            .headers_mut()
            .insert("origin", origin.parse().unwrap());
        request
            .headers_mut()
            .insert("x-forwarded-host", "attacker.test".parse().unwrap());
        assert_eq!(
            proxy.clone().oneshot(request).await.unwrap().status(),
            status
        );
    }
    assert!(matches!(
        RuntimeHttp::new(runtime(), web.path())
            .with_public_origin("https://user:password@app.test"),
        Err(HttpError::InvalidPublicOrigin)
    ));
}

#[tokio::test]
async fn only_the_trusted_server_context_can_authorize_a_restricted_action() {
    let web = web_root();
    let mut runtime = Runtime::new(
        Store::new(StoreSchema::default()),
        SecurityPolicy {
            enabled: true,
            documents: BTreeMap::from([(
                "Sales.Restricted".into(),
                BTreeSet::from(["Sales.Admin".into()]),
            )]),
            ..Default::default()
        },
    );
    runtime.register_action("Sales.Restricted", |_store: &mut Store, _: &Value| {
        Ok(json!("authorized"))
    });
    let adapter = RuntimeHttp::new(runtime, web.path());
    let mut forged = action_request("/api/action/Sales.Restricted");
    forged
        .headers_mut()
        .insert("x-user-roles", "Administrator".parse().unwrap());
    *forged.body_mut() = Body::from(
        r#"{"user":"admin","module_roles":["Sales.Admin"],"user_roles":["Administrator"]}"#,
    );
    let denied = adapter.router().unwrap().oneshot(forged).await.unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    assert_eq!(json_body(denied).await["error"], "not_authorized");
    let trusted = adapter.with_security_context(SecurityContext {
        user: Some("server-authenticated-user".into()),
        module_roles: BTreeSet::from(["Sales.Admin".into()]),
        ..Default::default()
    });
    let allowed = trusted
        .router()
        .unwrap()
        .oneshot(action_request("/api/action/Sales.Restricted"))
        .await
        .unwrap();
    assert_eq!(allowed.status(), StatusCode::OK);
    assert_eq!(json_body(allowed).await["result"], "authorized");
}

#[tokio::test]
async fn runtime_failures_have_distinct_http_statuses_and_machine_readable_codes() {
    let web = web_root();
    let mut runtime = runtime();
    runtime.register_action("Sales.Error", |_store: &mut Store, value: &Value| {
        Err(match value["error"].as_str().unwrap() {
            "entity" => RuntimeError::UnknownEntity("Sales.Missing".into()),
            "object" => RuntimeError::UnknownObject {
                entity: "Sales.Order".into(),
                id: "missing".into(),
            },
            "persistence" => RuntimeError::InvalidPersistence("bad snapshot".into()),
            _ => RuntimeError::Transaction("validation rejected".into()),
        })
    });
    let router = RuntimeHttp::new(runtime, web.path()).router().unwrap();
    for (error, status, code) in [
        ("entity", StatusCode::NOT_FOUND, "unknown_entity"),
        ("object", StatusCode::NOT_FOUND, "unknown_object"),
        (
            "transaction",
            StatusCode::UNPROCESSABLE_ENTITY,
            "transaction_failed",
        ),
        (
            "persistence",
            StatusCode::INTERNAL_SERVER_ERROR,
            "invalid_persistence",
        ),
    ] {
        let mut request = action_request("/api/action/Sales.Error");
        *request.body_mut() = Body::from(json!({"error":error}).to_string());
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(json_body(response).await["error"], code);
    }
    let response = router
        .oneshot(action_request("/api/action/Missing"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_body(response).await["error"], "unknown_action");
}

#[tokio::test(flavor = "current_thread")]
async fn slow_actions_do_not_block_health_checks_and_backpressure_is_explicit() {
    let web = web_root();
    let mut runtime = runtime();
    let (entered_tx, entered_rx) = oneshot::channel();
    let entered_tx = Mutex::new(Some(entered_tx));
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = Arc::new(Mutex::new(release_rx));
    runtime.register_action("Sales.Slow", move |_store: &mut Store, _: &Value| {
        entered_tx.lock().unwrap().take().unwrap().send(()).unwrap();
        release_rx
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .map_err(|error| RuntimeError::Transaction(error.to_string()))?;
        Ok(json!("finished"))
    });
    let router = RuntimeHttp::new(runtime, web.path()).router().unwrap();
    let slow = tokio::spawn(
        router
            .clone()
            .oneshot(action_request("/api/action/Sales.Slow")),
    );
    tokio::time::timeout(Duration::from_secs(2), entered_rx)
        .await
        .unwrap()
        .unwrap();
    let health = tokio::time::timeout(
        Duration::from_secs(1),
        router
            .clone()
            .oneshot(Request::get("/api/health").body(Body::empty()).unwrap()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
    let busy = router
        .clone()
        .oneshot(action_request("/api/action/Sales.Echo"))
        .await
        .unwrap();
    assert_eq!(busy.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(busy.headers()["retry-after"], "1");
    assert_eq!(json_body(busy).await["error"], "runtime_busy");
    release_tx.send(()).unwrap();
    assert_eq!(slow.await.unwrap().unwrap().status(), StatusCode::OK);
    assert_eq!(
        router
            .oneshot(action_request("/api/action/Sales.Echo"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn panicking_actions_fail_closed_and_do_not_take_down_health_checks() {
    let web = web_root();
    let mut runtime = runtime();
    runtime.register_action(
        "Sales.Panic",
        |_store: &mut Store, _: &Value| -> mxrs_runtime::Result<Value> {
            panic!("deliberate action failure")
        },
    );
    let router = RuntimeHttp::new(runtime, web.path()).router().unwrap();
    let failed = router
        .clone()
        .oneshot(action_request("/api/action/Sales.Panic"))
        .await
        .unwrap();
    assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(json_body(failed).await["error"], "runtime_action_failed");
    let poisoned = router
        .clone()
        .oneshot(action_request("/api/action/Sales.Echo"))
        .await
        .unwrap();
    assert_eq!(poisoned.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(json_body(poisoned).await["error"], "runtime_lock_poisoned");
    assert_eq!(
        router
            .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

async fn socket_request(address: std::net::SocketAddr, request: &str) -> String {
    let mut socket = TcpStream::connect(address).await.unwrap();
    socket.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    tokio::time::timeout(Duration::from_secs(3), socket.read_to_string(&mut response))
        .await
        .unwrap()
        .unwrap();
    response
}

#[tokio::test]
async fn a_real_listener_serves_actions_and_stops_accepting_after_graceful_shutdown() {
    let web = web_root();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(RuntimeHttp::new(runtime(), web.path()).serve_listener(
        listener,
        async {
            let _ = shutdown_rx.await;
        },
    ));
    let health = socket_request(
        address,
        &format!("GET /api/health HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"),
    )
    .await;
    assert!(health.starts_with("HTTP/1.1 200 OK"));
    assert!(health.contains(r#""runtime":"mxrs""#));
    let body = r#"{"value":42}"#;
    let action = socket_request(address, &format!("POST /api/action/Sales.Echo HTTP/1.1\r\nHost: {address}\r\nOrigin: http://{address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())).await;
    assert!(action.starts_with("HTTP/1.1 200 OK"));
    assert!(action.contains(r#"{"result":{"value":42}}"#));
    shutdown_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(TcpStream::connect(address).await.is_err());
}

#[tokio::test]
async fn shutdown_and_bind_failures_are_observable_to_the_server_owner() {
    let web = web_root();
    RuntimeHttp::new(runtime(), web.path())
        .serve_with_shutdown("127.0.0.1:0".parse().unwrap(), async {})
        .await
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    assert!(matches!(
        RuntimeHttp::new(runtime(), web.path()).serve(address).await,
        Err(HttpError::Bind(_))
    ));
    assert!(matches!(
        RuntimeHttp::new(runtime(), web.path().join("missing"))
            .serve_with_shutdown(address, async {})
            .await,
        Err(HttpError::MissingWebRoot(_))
    ));
}

#[tokio::test]
async fn graceful_shutdown_drains_an_in_flight_action_before_returning() {
    let web = web_root();
    let mut runtime = runtime();
    let (entered_tx, entered_rx) = oneshot::channel();
    let entered_tx = Mutex::new(Some(entered_tx));
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    runtime.register_action("Sales.Drain", move |_store: &mut Store, _: &Value| {
        entered_tx.lock().unwrap().take().unwrap().send(()).unwrap();
        release_rx
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .map_err(|error| RuntimeError::Transaction(error.to_string()))?;
        Ok(json!("drained"))
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(RuntimeHttp::new(runtime, web.path()).serve_listener(
        listener,
        async {
            let _ = shutdown_rx.await;
        },
    ));
    let request = tokio::spawn(async move {
        socket_request(address, &format!("POST /api/action/Sales.Drain HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}")).await
    });
    tokio::time::timeout(Duration::from_secs(2), entered_rx)
        .await
        .unwrap()
        .unwrap();
    shutdown_tx.send(()).unwrap();
    tokio::task::yield_now().await;
    assert!(
        !server.is_finished(),
        "shutdown discarded an in-flight action"
    );
    release_tx.send(()).unwrap();
    let response = request.await.unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"));
    assert!(response.contains(r#""result":"drained""#));
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn cancelling_a_response_does_not_release_the_running_action_slot() {
    let web = web_root();
    let mut runtime = runtime();
    let (entered_tx, entered_rx) = oneshot::channel();
    let entered_tx = Mutex::new(Some(entered_tx));
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    runtime.register_action("Sales.Cancelled", move |_store: &mut Store, _: &Value| {
        entered_tx.lock().unwrap().take().unwrap().send(()).unwrap();
        release_rx
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .map_err(|error| RuntimeError::Transaction(error.to_string()))?;
        Ok(Value::Null)
    });
    let adapter = RuntimeHttp::new(runtime, web.path());
    let router = adapter.router().unwrap();
    let request = tokio::spawn(
        router
            .clone()
            .oneshot(action_request("/api/action/Sales.Cancelled")),
    );
    tokio::time::timeout(Duration::from_secs(2), entered_rx)
        .await
        .unwrap()
        .unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    let busy = router
        .oneshot(action_request("/api/action/Sales.Echo"))
        .await
        .unwrap();
    assert_eq!(busy.status(), StatusCode::SERVICE_UNAVAILABLE);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut server = tokio::spawn(adapter.serve_listener(listener, async {}));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut server)
            .await
            .is_err(),
        "shutdown forgot the disconnected action"
    );
    release_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
