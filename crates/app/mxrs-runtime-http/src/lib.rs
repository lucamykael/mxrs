//! HTTP boundary for [`mxrs_runtime::Runtime`].
//!
//! Authentication information is server-owned: the browser request body is
//! passed only as action arguments and can never grant itself roles. Use
//! [`RuntimeHttp::with_security_context`] after an authentication adapter has
//! established a trusted context. The default is anonymous and therefore
//! fails closed whenever project security is enabled.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::extract::{DefaultBodyLimit, Path as RoutePath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use mxrs_runtime::{Runtime, RuntimeError, SecurityContext};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tower_http::services::ServeDir;

const MAX_ACTION_BODY_BYTES: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("web root is not a directory: {0}")]
    MissingWebRoot(String),
    #[error("cannot bind runtime HTTP listener: {0}")]
    Bind(#[source] std::io::Error),
    #[error("runtime HTTP server failed: {0}")]
    Serve(#[source] std::io::Error),
}

pub type Result<T> = std::result::Result<T, HttpError>;

#[derive(Clone)]
struct AppState {
    runtime: Arc<Mutex<Runtime>>,
    context: SecurityContext,
}

/// A shareable application boundary around one transactional runtime.
pub struct RuntimeHttp {
    state: AppState,
    web_root: PathBuf,
}

impl RuntimeHttp {
    pub fn new(runtime: Runtime, web_root: impl Into<PathBuf>) -> Self {
        Self {
            state: AppState {
                runtime: Arc::new(Mutex::new(runtime)),
                context: SecurityContext::default(),
            },
            web_root: web_root.into(),
        }
    }

    /// Installs a context obtained from a trusted server-side authentication
    /// boundary. No request header or JSON field is interpreted as a role.
    pub fn with_security_context(mut self, context: SecurityContext) -> Self {
        self.state.context = context;
        self
    }

    pub fn router(&self) -> Result<Router> {
        validate_web_root(&self.web_root)?;
        let static_files = ServeDir::new(&self.web_root).append_index_html_on_directories(true);
        Ok(Router::new()
            .route("/api/health", get(health))
            .route("/api/{kind}/{handler}", post(invoke))
            .layer(DefaultBodyLimit::max(MAX_ACTION_BODY_BYTES))
            .fallback_service(static_files)
            .with_state(self.state.clone()))
    }

    pub async fn serve(self, address: SocketAddr) -> Result<()> {
        let router = self.router()?;
        let listener = TcpListener::bind(address).await.map_err(HttpError::Bind)?;
        axum::serve(listener, router)
            .await
            .map_err(HttpError::Serve)
    }
}

fn validate_web_root(path: &Path) -> Result<()> {
    if path.is_dir() {
        Ok(())
    } else {
        Err(HttpError::MissingWebRoot(path.display().to_string()))
    }
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok", "runtime": "mxrs" }))
}

async fn invoke(
    State(state): State<AppState>,
    RoutePath((kind, handler)): RoutePath<(String, String)>,
    Json(arguments): Json<Value>,
) -> Response {
    if !matches!(kind.as_str(), "action" | "microflow" | "nanoflow") {
        return error_response(StatusCode::NOT_FOUND, "unknown_action_kind", &kind);
    }
    let result = match state.runtime.lock() {
        Ok(mut runtime) => runtime.invoke(&handler, &arguments, &state.context),
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "runtime_lock_poisoned",
                "runtime state is unavailable",
            );
        }
    };
    match result {
        Ok(value) => Json(json!({ "result": value })).into_response(),
        Err(error) => runtime_error_response(error),
    }
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: &'a str,
    message: &'a str,
}

fn runtime_error_response(error: RuntimeError) -> Response {
    let status = match error {
        RuntimeError::NotAuthorized { .. } => StatusCode::FORBIDDEN,
        RuntimeError::UnknownAction(_)
        | RuntimeError::UnknownEntity(_)
        | RuntimeError::UnknownObject { .. } => StatusCode::NOT_FOUND,
        RuntimeError::Transaction(_) => StatusCode::UNPROCESSABLE_ENTITY,
    };
    let code = match error {
        RuntimeError::NotAuthorized { .. } => "not_authorized",
        RuntimeError::UnknownAction(_) => "unknown_action",
        RuntimeError::UnknownEntity(_) => "unknown_entity",
        RuntimeError::UnknownObject { .. } => "unknown_object",
        RuntimeError::Transaction(_) => "transaction_failed",
    };
    error_response(status, code, &error.to_string())
}

fn error_response(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(ErrorBody {
            error: code,
            message,
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt as _;
    use mxrs_runtime::{SecurityPolicy, Store, StoreSchema};
    use std::collections::{BTreeMap, BTreeSet};
    use tower::ServiceExt as _;

    fn web_root() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("index.html"), "<h1>MXRS</h1>").unwrap();
        directory
    }

    #[tokio::test]
    async fn serves_static_assets_health_and_runtime_actions() {
        let web = web_root();
        let mut runtime = Runtime::new(
            Store::new(StoreSchema::default().entity("Sales.Order", BTreeMap::new(), false)),
            SecurityPolicy::default(),
        );
        runtime.register_action("Sales.ACT_Echo", |_store: &mut Store, value: &Value| {
            Ok(value.clone())
        });
        let router = RuntimeHttp::new(runtime, web.path()).router().unwrap();

        let health = router
            .clone()
            .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(health.status(), StatusCode::OK);

        let action = router
            .clone()
            .oneshot(
                Request::post("/api/microflow/Sales.ACT_Echo")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"value":42}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(action.status(), StatusCode::OK);
        let body = action.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["result"]["value"],
            42
        );

        let index = router
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(index.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn anonymous_requests_cannot_self_assign_roles_in_the_json_body() {
        let web = web_root();
        let policy = SecurityPolicy {
            enabled: true,
            documents: BTreeMap::from([(
                "Sales.ACT_Admin".into(),
                BTreeSet::from(["Sales.Admin".into()]),
            )]),
            ..Default::default()
        };
        let mut runtime = Runtime::new(Store::new(StoreSchema::default()), policy);
        runtime.register_action("Sales.ACT_Admin", |_store: &mut Store, _value: &Value| {
            Ok(json!(true))
        });
        let router = RuntimeHttp::new(runtime, web.path()).router().unwrap();
        let response = router
            .oneshot(
                Request::post("/api/microflow/Sales.ACT_Admin")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"user_roles":["Administrator"]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn missing_web_root_fails_before_binding_a_socket() {
        let runtime = Runtime::new(
            Store::new(StoreSchema::default()),
            SecurityPolicy::default(),
        );
        let error = RuntimeHttp::new(runtime, "/definitely/missing")
            .router()
            .unwrap_err();
        assert!(matches!(error, HttpError::MissingWebRoot(_)));
    }
}
