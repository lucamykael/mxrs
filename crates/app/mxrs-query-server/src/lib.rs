//! Loopback-only JSON API for read-only queries against a synchronized
//! project database.
//!
//! Ports `Mxrb::Oql::Server`: one POST endpoint accepting a JSON object with
//! exactly one of `sql` or `oql` plus optional `params`. OQL is translated
//! through the safe [`mxrs_oql`] subset before execution; raw SQL is passed to
//! the executor, whose own read-only statement validation is the enforcement
//! boundary. The server never opens a database itself — callers inject a
//! [`QueryRows`] executor, which keeps the HTTP contract testable offline.

use std::future::{Future, pending};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode, header};
use axum::response::Response;
use serde_json::{Map, Value, json};
use tokio::net::TcpListener;

pub const MAX_BODY_BYTES: usize = 1_048_576;

#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    /// The request itself is malformed (bad statement, bad parameters).
    /// Rendered as HTTP 400 `invalid_request`, like mxrb's `ArgumentError`.
    #[error("{0}")]
    InvalidRequest(String),
    /// The database rejected or failed the query. Rendered as HTTP 422
    /// `query_failed`, like mxrb's `StandardError` fallback.
    #[error("{0}")]
    Failed(String),
}

/// One read-only query against the project database. Rows are column-name to
/// value maps; a SQL `NULL` is `Value::Null` and every other value is the
/// database's own string rendering, mirroring mxrb's CSV-derived rows.
pub trait QueryRows: Send + Sync {
    fn query_rows(
        &self,
        sql: &str,
        params: &Map<String, Value>,
    ) -> Result<Vec<Map<String, Value>>, QueryError>;
}

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("the query server must bind to a loopback address")]
    NotLoopback,
    #[error("cannot bind query server listener: {0}")]
    Bind(#[source] std::io::Error),
    #[error("query server failed: {0}")]
    Serve(#[source] std::io::Error),
}

pub struct QueryServer {
    executor: Arc<dyn QueryRows>,
    host: String,
    port: u16,
}

impl QueryServer {
    pub fn new(
        executor: Arc<dyn QueryRows>,
        host: impl Into<String>,
        port: u16,
    ) -> Result<Self, ServerError> {
        let host = host.into();
        if !matches!(host.as_str(), "127.0.0.1" | "::1" | "localhost") {
            return Err(ServerError::NotLoopback);
        }
        Ok(Self {
            executor,
            host,
            port,
        })
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Every path dispatches to the same handler, exactly like mxrb's shared
    /// HTTP server: the method check, not routing, produces 405.
    pub fn router(&self) -> Router {
        Router::new()
            .fallback(dispatch)
            .with_state(self.executor.clone())
    }

    pub async fn serve(self) -> Result<(), ServerError> {
        self.serve_with_shutdown(pending()).await
    }

    pub async fn serve_with_shutdown(
        self,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), ServerError> {
        let address: SocketAddr = match self.host.as_str() {
            "::1" => ([0u16, 0, 0, 0, 0, 0, 0, 1], self.port).into(),
            _ => ([127, 0, 0, 1], self.port).into(),
        };
        let listener = TcpListener::bind(address)
            .await
            .map_err(ServerError::Bind)?;
        self.serve_listener(listener, shutdown).await
    }

    /// Accepting an already-bound listener lets callers discover an ephemeral
    /// port before starting acceptance tests.
    pub async fn serve_listener(
        self,
        listener: TcpListener,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), ServerError> {
        let router = self.router();
        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown)
            .await
            .map_err(ServerError::Serve)
    }
}

/// Runs one already-parsed request payload. Public so the CLI can offer the
/// same contract without HTTP if it ever needs to, and so tests can bypass
/// the transport.
pub fn execute(executor: &dyn QueryRows, payload: &Value) -> Value {
    let started = Instant::now();
    match query_sql(payload) {
        Ok((sql, warnings, params)) => match executor.query_rows(&sql, &params) {
            Ok(rows) => {
                json!({
                    "ok": true,
                    "row_count": rows.len(),
                    "rows": rows,
                    "elapsed_ms": elapsed_ms(started),
                    "warnings": warnings,
                })
            }
            Err(error) => error_payload(&error, started),
        },
        Err(message) => error_payload(&QueryError::InvalidRequest(message), started),
    }
}

/// A statement ready to execute: SQL, translator warnings, bound parameters.
type PreparedQuery = (String, Vec<String>, Map<String, Value>);

fn query_sql(payload: &Value) -> Result<PreparedQuery, String> {
    let Value::Object(payload) = payload else {
        return Err("JSON body must be an object".to_string());
    };
    let sql = loose_string(payload.get("sql"));
    let oql = loose_string(payload.get("oql"));
    if sql.is_empty() == oql.is_empty() {
        return Err("provide exactly one of sql or oql".to_string());
    }
    let params = match payload.get("params") {
        None => Map::new(),
        Some(Value::Object(params)) => params.clone(),
        Some(_) => return Err("params must be a JSON object".to_string()),
    };
    if !sql.is_empty() {
        return Ok((sql, Vec::new(), params));
    }
    translate_oql(&oql, params)
}

fn translate_oql(oql: &str, params: Map<String, Value>) -> Result<PreparedQuery, String> {
    let projection = mxrs_oql::translate(oql, mxrs_oql::Dialect::PostgreSql);
    let Some(sql) = projection.sql else {
        return Err(projection.warnings.join("; "));
    };
    let mut expected = projection.parameters;
    expected.sort();
    let mut provided: Vec<&String> = params.keys().collect();
    provided.sort();
    if provided != expected.iter().collect::<Vec<_>>() {
        return Err(format!(
            "params must match OQL parameters: {}",
            expected.join(", ")
        ));
    }
    Ok((sql, projection.warnings, params))
}

/// Mirrors Ruby's `to_s` closely enough for the XOR check: only a missing
/// key, `null`, or an empty string counts as "not provided". Non-string
/// scalars become their rendering and are rejected downstream by the
/// executor's statement validation, as they are in mxrb.
fn loose_string(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(value)) => value.clone(),
        Some(Value::Bool(value)) => value.to_string(),
        Some(Value::Number(value)) => value.to_string(),
        Some(value) => value.to_string(),
    }
}

fn elapsed_ms(started: Instant) -> f64 {
    (started.elapsed().as_secs_f64() * 1_000_000.0).round() / 1000.0
}

fn error_payload(error: &QueryError, started: Instant) -> Value {
    let code = match error {
        QueryError::InvalidRequest(_) => "invalid_request",
        QueryError::Failed(_) => "query_failed",
    };
    json!({
        "ok": false,
        "error": { "code": code, "message": error.to_string() },
        "elapsed_ms": elapsed_ms(started),
    })
}

async fn dispatch(State(executor): State<Arc<dyn QueryRows>>, request: Request) -> Response {
    if request.method() != Method::POST {
        return render(
            StatusCode::METHOD_NOT_ALLOWED,
            json!({
                "ok": false,
                "error": { "code": "method_not_allowed", "message": "use POST /query" },
            }),
        );
    }
    let bytes = match to_bytes(request.into_body(), MAX_BODY_BYTES + 1).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return request_too_large();
        }
    };
    if bytes.len() > MAX_BODY_BYTES {
        return request_too_large();
    }
    let payload: Value = match serde_json::from_slice(&bytes) {
        Ok(payload) => payload,
        Err(error) => {
            return render(
                StatusCode::BAD_REQUEST,
                json!({
                    "ok": false,
                    "error": { "code": "invalid_json", "message": error.to_string() },
                }),
            );
        }
    };
    let result = tokio::task::block_in_place(|| execute(executor.as_ref(), &payload));
    let status = if result["ok"] == Value::Bool(true) {
        StatusCode::OK
    } else if result["error"]["code"] == "invalid_request" {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::UNPROCESSABLE_ENTITY
    };
    render(status, result)
}

fn request_too_large() -> Response {
    render(
        StatusCode::PAYLOAD_TOO_LARGE,
        json!({
            "ok": false,
            "error": { "code": "request_too_large", "message": "request body exceeds 1 MiB" },
        }),
    )
}

fn render(status: StatusCode, payload: Value) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .body(Body::from(payload.to_string()))
        .expect("static response parts are valid")
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;

    type FakeOutcome = Result<Vec<Map<String, Value>>, fn() -> QueryError>;

    struct FakeRows(FakeOutcome);

    impl QueryRows for FakeRows {
        fn query_rows(
            &self,
            _sql: &str,
            _params: &Map<String, Value>,
        ) -> Result<Vec<Map<String, Value>>, QueryError> {
            match &self.0 {
                Ok(rows) => Ok(rows.clone()),
                Err(error) => Err(error()),
            }
        }
    }

    fn server(executor: impl QueryRows + 'static) -> QueryServer {
        QueryServer::new(Arc::new(executor), "127.0.0.1", 4567).unwrap()
    }

    async fn post(router: Router, body: &str) -> (StatusCode, Value) {
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/query")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[test]
    fn the_server_only_binds_loopback_hosts() {
        for host in ["127.0.0.1", "::1", "localhost"] {
            assert!(QueryServer::new(Arc::new(FakeRows(Ok(Vec::new()))), host, 4567).is_ok());
        }
        assert!(matches!(
            QueryServer::new(Arc::new(FakeRows(Ok(Vec::new()))), "0.0.0.0", 4567),
            Err(ServerError::NotLoopback)
        ));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn non_post_requests_are_rejected_with_405() {
        let response = server(FakeRows(Ok(Vec::new())))
            .router()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/query")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let payload: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(payload["error"]["code"], "method_not_allowed");
        assert_eq!(payload["error"]["message"], "use POST /query");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_oversized_body_is_rejected_before_parsing() {
        let body = format!("{{\"sql\": \"SELECT '{}'\"}}", "x".repeat(MAX_BODY_BYTES));
        let (status, payload) = post(server(FakeRows(Ok(Vec::new()))).router(), &body).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(payload["error"]["code"], "request_too_large");
        assert_eq!(payload["error"]["message"], "request body exceeds 1 MiB");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn malformed_json_is_a_400_invalid_json() {
        let (status, payload) = post(server(FakeRows(Ok(Vec::new()))).router(), "not json").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(payload["error"]["code"], "invalid_json");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_sql_query_returns_rows_counts_timing_and_no_warnings() {
        let mut row = Map::new();
        row.insert("name".to_string(), Value::String("Sales".to_string()));
        row.insert("total".to_string(), Value::Null);
        let (status, payload) = post(
            server(FakeRows(Ok(vec![row]))).router(),
            r#"{"sql": "SELECT 1"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(payload["ok"], true);
        assert_eq!(payload["row_count"], 1);
        assert_eq!(payload["rows"][0]["name"], "Sales");
        assert_eq!(payload["rows"][0]["total"], Value::Null);
        assert_eq!(payload["warnings"], json!([]));
        assert!(payload["elapsed_ms"].is_number());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn requests_must_name_exactly_one_of_sql_or_oql() {
        let router = server(FakeRows(Ok(Vec::new()))).router();
        for body in [
            r#"{}"#,
            r#"{"sql": "", "oql": ""}"#,
            r#"{"sql": "SELECT 1", "oql": "FROM Sales.Order SELECT Total"}"#,
            r#"[1]"#,
            r#"{"sql": "SELECT 1", "params": []}"#,
        ] {
            let (status, payload) = post(router.clone(), body).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(payload["error"]["code"], "invalid_request", "{body}");
        }
        let (_, payload) = post(router.clone(), r#"{"sql": "", "oql": ""}"#).await;
        assert_eq!(
            payload["error"]["message"],
            "provide exactly one of sql or oql"
        );
        let (_, payload) = post(router.clone(), r#"[1]"#).await;
        assert_eq!(payload["error"]["message"], "JSON body must be an object");
        let (_, payload) = post(router, r#"{"sql": "SELECT 1", "params": []}"#).await;
        assert_eq!(payload["error"]["message"], "params must be a JSON object");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn oql_is_translated_and_its_parameters_must_match_exactly() {
        let capture = Arc::new(Capture::default());
        let router = QueryServer::new(capture.clone(), "127.0.0.1", 4567)
            .unwrap()
            .router();
        let (status, payload) = post(
            router.clone(),
            r#"{"oql": "FROM Sales.Order AS o WHERE o/Total > $minimum SELECT o/Total AS total", "params": {"minimum": 5}}"#,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{payload}");
        {
            let calls = capture.calls.lock().unwrap();
            let (sql, params) = calls.first().expect("executor called");
            assert!(sql.contains(":minimum"), "{sql}");
            assert_eq!(params["minimum"], 5);
        }

        let (status, payload) = post(
            router,
            r#"{"oql": "FROM Sales.Order AS o WHERE o/Total > $minimum SELECT o/Total AS total", "params": {"wrong": 5}}"#,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            payload["error"]["message"],
            "params must match OQL parameters: minimum"
        );
    }

    #[derive(Default)]
    struct Capture {
        calls: std::sync::Mutex<Vec<(String, Map<String, Value>)>>,
    }

    impl QueryRows for Capture {
        fn query_rows(
            &self,
            sql: &str,
            params: &Map<String, Value>,
        ) -> Result<Vec<Map<String, Value>>, QueryError> {
            self.calls
                .lock()
                .unwrap()
                .push((sql.to_string(), params.clone()));
            Ok(Vec::new())
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn unsupported_oql_reports_the_translator_warnings() {
        let (status, payload) = post(
            server(FakeRows(Ok(Vec::new()))).router(),
            r#"{"oql": "DELETE FROM Sales.Order"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(payload["error"]["code"], "invalid_request");
        assert!(!payload["error"]["message"].as_str().unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn executor_failures_are_422_and_request_errors_are_400() {
        let (status, payload) = post(
            server(FakeRows(Err(|| {
                QueryError::Failed("relation missing".into())
            })))
            .router(),
            r#"{"sql": "SELECT 1"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(payload["error"]["code"], "query_failed");
        assert_eq!(payload["error"]["message"], "relation missing");

        let (status, payload) = post(
            server(FakeRows(Err(|| {
                QueryError::InvalidRequest("missing query parameter: minimum".into())
            })))
            .router(),
            r#"{"sql": "SELECT :minimum"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(payload["error"]["code"], "invalid_request");
    }
}
