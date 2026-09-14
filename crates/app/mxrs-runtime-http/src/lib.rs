//! HTTP boundary for [`mxrs_runtime::Runtime`].
//!
//! Authentication information is server-owned: the browser request body is
//! passed only as action arguments and can never grant itself roles. Use
//! [`RuntimeHttp::with_security_context`] after an authentication adapter has
//! established a trusted context. The default is anonymous and therefore
//! fails closed whenever project security is enabled.
//!
//! This is not a session/authentication implementation: one trusted context is
//! shared by this adapter. Deployments behind a reverse proxy must configure
//! [`RuntimeHttp::with_public_origin`] and must not trust client-supplied
//! forwarding headers. Without a pinned origin, browser actions are limited
//! to HTTP localhost and loopback-IP origins to prevent DNS rebinding.
//! Actions are serialized and excess concurrent requests
//! receive 503; running synchronous actions cannot be forcibly cancelled.

mod origin;
mod static_files;

use std::future::{Future, pending};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::{DefaultBodyLimit, Path as RoutePath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use mxrs_runtime::{Runtime, RuntimeError, SecurityContext};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

const MAX_ACTION_BODY_BYTES: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("web root is not a directory: {0}")]
    MissingWebRoot(String),
    #[error("cannot read public web assets: {0}")]
    WebAssets(#[source] std::io::Error),
    #[error("web asset path is not safe to publish: {0}")]
    UnsafeWebAsset(String),
    #[error("public web assets exceed the {0}-byte snapshot limit")]
    WebAssetsTooLarge(usize),
    #[error("public origin must be an HTTP(S) origin without credentials, path, query or fragment")]
    InvalidPublicOrigin,
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
    action_slots: Arc<Semaphore>,
    public_origin: Option<origin::Origin>,
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
                action_slots: Arc::new(Semaphore::new(1)),
                public_origin: None,
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

    /// Pins the browser-facing origin when TLS termination or a reverse proxy
    /// makes the listener's HTTP authority different from the public one.
    pub fn with_public_origin(mut self, origin: &str) -> Result<Self> {
        self.state.public_origin = Some(origin::Origin::parse(origin)?);
        Ok(self)
    }

    pub fn router(&self) -> Result<Router> {
        let files = static_files::Snapshot::load(&self.web_root)?;
        Ok(Router::new()
            .route("/api/health", get(health))
            .route("/api/{kind}/{handler}", post(invoke))
            .layer(DefaultBodyLimit::max(MAX_ACTION_BODY_BYTES))
            .fallback(move |request| files.clone().respond(request))
            .with_state(self.state.clone()))
    }

    pub async fn serve(self, address: SocketAddr) -> Result<()> {
        self.serve_with_shutdown(address, pending()).await
    }

    pub async fn serve_with_shutdown(
        self,
        address: SocketAddr,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> Result<()> {
        let router = self.router()?;
        let listener = TcpListener::bind(address).await.map_err(HttpError::Bind)?;
        self.serve_bound(listener, router, shutdown).await
    }

    /// Accepting an already-bound listener supports socket activation and lets
    /// callers discover an ephemeral port before starting acceptance tests.
    pub async fn serve_listener(
        self,
        listener: TcpListener,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> Result<()> {
        let router = self.router()?;
        self.serve_bound(listener, router, shutdown).await
    }

    async fn serve_bound(
        self,
        listener: TcpListener,
        router: Router,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> Result<()> {
        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown)
            .await
            .map_err(HttpError::Serve)?;
        // A disconnected caller can drop its response future while its
        // synchronous transaction still runs. Drain that work as well.
        let _permit = self.state.action_slots.acquire().await;
        Ok(())
    }
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok", "runtime": "mxrs" }))
}

async fn invoke(
    State(state): State<AppState>,
    RoutePath((kind, handler)): RoutePath<(String, String)>,
    headers: HeaderMap,
    Json(arguments): Json<Value>,
) -> Response {
    if !origin::request_allowed(&headers, state.public_origin.as_ref()) {
        return error_response(
            StatusCode::FORBIDDEN,
            "cross_origin_request",
            "runtime actions require the application's own origin",
        );
    }
    if !matches!(kind.as_str(), "action" | "microflow" | "nanoflow") {
        return error_response(StatusCode::NOT_FOUND, "unknown_action_kind", &kind);
    }
    let permit = match state.action_slots.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            let mut response = error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "runtime_busy",
                "another runtime action is still running",
            );
            response
                .headers_mut()
                .insert("retry-after", "1".parse().unwrap());
            return response;
        }
    };
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        match state.runtime.lock() {
            Ok(mut runtime) => match runtime.invoke(&handler, &arguments, &state.context) {
                Ok(value) => Json(json!({ "result": value })).into_response(),
                Err(error) => runtime_error_response(error),
            },
            Err(_) => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "runtime_lock_poisoned",
                "runtime state is unavailable",
            ),
        }
    })
    .await
    {
        Ok(response) => response,
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime_action_failed",
            "runtime action could not complete",
        ),
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
        RuntimeError::InvalidPersistence(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let code = match error {
        RuntimeError::NotAuthorized { .. } => "not_authorized",
        RuntimeError::UnknownAction(_) => "unknown_action",
        RuntimeError::UnknownEntity(_) => "unknown_entity",
        RuntimeError::UnknownObject { .. } => "unknown_object",
        RuntimeError::Transaction(_) => "transaction_failed",
        RuntimeError::InvalidPersistence(_) => "invalid_persistence",
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
        let adapter = RuntimeHttp::new(runtime, web.path());
        let router = adapter.router().unwrap();
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
        let trusted = adapter.with_security_context(SecurityContext {
            module_roles: BTreeSet::from(["Sales.Admin".into()]),
            ..Default::default()
        });
        let allowed = trusted
            .router()
            .unwrap()
            .oneshot(
                Request::post("/api/microflow/Sales.ACT_Admin")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(allowed.status(), StatusCode::OK);
        assert_eq!(
            serde_json::from_slice::<Value>(
                &allowed.into_body().collect().await.unwrap().to_bytes()
            )
            .unwrap()["result"],
            true
        );
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
