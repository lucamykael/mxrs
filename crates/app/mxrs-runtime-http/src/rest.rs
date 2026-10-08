//! The model's published REST services, served by the runtime.
//!
//! Each operation is a route: its method at its service's path, its
//! resource's name and its own path. A request signs in as its service asks
//! — anyone, as the project's guest, or HTTP Basic against the model's own
//! accounts, holding one of the service's module roles — and then runs the
//! operation's action with what the request carries: its path parameters,
//! its query string, its body and its URI. The action binds those to the
//! microflow and answers the operation's document, under the status the
//! flow chose.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Query, RawPathParams};
use axum::http::header::{AUTHORIZATION, WWW_AUTHENTICATE};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodFilter, MethodRouter, on};
use mxrs_runtime::{Runtime, SecurityContext};
use serde_json::{Value, json};
use tokio::sync::Semaphore;

use crate::{error_response, runtime_error_response};

/// How a published service learns who is calling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestAuthentication {
    /// Anyone, as the project's guest.
    Public,
    /// HTTP Basic against the model's accounts; the caller must hold one of
    /// the module roles. `realm` is what a `401` challenges with.
    Basic {
        realm: String,
        allowed_roles: Vec<String>,
    },
    /// Authentication this runtime does not offer, as the model states it.
    /// Every operation refuses with `501` rather than serve the service to
    /// anyone.
    Unsupported(String),
}

/// One operation of a published service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestRoute {
    /// `GET`, `POST`, …
    pub method: Method,
    /// `/rest/orders/v1/order/{id}`.
    pub path: String,
    /// The runtime action that runs the operation.
    pub action: String,
    pub authentication: RestAuthentication,
}

/// Who a request signs in as: the model's accounts and roles.
pub trait RestAccounts: Send + Sync {
    /// The caller a user name and password sign in as.
    fn sign_in(&self, user: &str, password: &str) -> Option<SecurityContext>;
    /// Who a request with no credentials is.
    fn anonymous(&self) -> SecurityContext;
    /// Whether `caller` holds one of `roles`.
    fn allows(&self, caller: &SecurityContext, roles: &[&str]) -> bool;
}

/// `router` serving every route.
pub(crate) fn serve<S: Clone + Send + Sync + 'static>(
    router: Router<S>,
    routes: &[RestRoute],
    accounts: Arc<dyn RestAccounts>,
    runtime: Arc<Mutex<Runtime>>,
    slots: Arc<Semaphore>,
) -> Router<S> {
    let mut paths: BTreeMap<String, MethodRouter<S>> = BTreeMap::new();
    for route in routes {
        let Ok(filter) = MethodFilter::try_from(route.method.clone()) else {
            continue;
        };
        let operation = Arc::new(Operation {
            action: route.action.clone(),
            authentication: route.authentication.clone(),
            accounts: accounts.clone(),
            runtime: runtime.clone(),
            slots: slots.clone(),
        });
        let handler = on(
            filter,
            move |path: RawPathParams,
                  Query(query): Query<BTreeMap<String, String>>,
                  uri: Uri,
                  headers: HeaderMap,
                  body: Bytes| {
                let operation = operation.clone();
                async move { operation.answer(path, query, uri, headers, body).await }
            },
        );
        let method_router = match paths.remove(&route.path) {
            Some(existing) => existing.merge(handler),
            None => handler,
        };
        paths.insert(route.path.clone(), method_router);
    }
    paths.into_iter().fold(router, |router, (path, handler)| {
        router.route(&path, handler)
    })
}

struct Operation {
    action: String,
    authentication: RestAuthentication,
    accounts: Arc<dyn RestAccounts>,
    runtime: Arc<Mutex<Runtime>>,
    slots: Arc<Semaphore>,
}

impl Operation {
    async fn answer(
        &self,
        path: RawPathParams,
        query: BTreeMap<String, String>,
        uri: Uri,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        let caller = match self.caller(&headers) {
            Ok(caller) => caller,
            Err(response) => return response,
        };
        let path: BTreeMap<String, String> = path
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        let body = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).into_owned()))
        };
        let arguments = json!({
            "path": path,
            "query": query,
            "body": body,
            "uri": uri.to_string(),
        });
        // Operations wait their turn on the one runtime, as a page's data
        // does.
        let Ok(permit) = self.slots.clone().acquire_owned().await else {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "runtime_stopping",
                "the runtime is shutting down",
            );
        };
        let runtime = self.runtime.clone();
        let action = self.action.clone();
        let answered = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut runtime = runtime
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            runtime.invoke_published(&action, &arguments, &caller)
        })
        .await;
        match answered {
            Ok(Ok(answer)) => respond(answer),
            Ok(Err(error)) => runtime_error_response(error),
            Err(_) => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "operation_failed",
                "the operation could not complete",
            ),
        }
    }

    /// Who the request is, as the service asks — or the response refusing
    /// it.
    fn caller(&self, headers: &HeaderMap) -> Result<SecurityContext, Response> {
        match &self.authentication {
            RestAuthentication::Public => Ok(self.accounts.anonymous()),
            RestAuthentication::Unsupported(declared) => Err(error_response(
                StatusCode::NOT_IMPLEMENTED,
                "unsupported_authentication",
                &format!("the service requires {declared}, which this runtime does not offer"),
            )),
            RestAuthentication::Basic {
                realm,
                allowed_roles,
            } => {
                // Missing and wrong credentials are one answer on purpose:
                // telling them apart tells a caller which user names exist.
                let Some(caller) = headers
                    .get(AUTHORIZATION)
                    .and_then(|header| header.to_str().ok())
                    .and_then(crate::basic_credentials)
                    .and_then(|(user, password)| self.accounts.sign_in(&user, &password))
                else {
                    let mut response = error_response(
                        StatusCode::UNAUTHORIZED,
                        "unauthenticated",
                        "the service requires HTTP Basic credentials",
                    );
                    if let Ok(challenge) =
                        format!("Basic realm=\"{}\"", realm.replace('"', "'")).parse()
                    {
                        response.headers_mut().insert(WWW_AUTHENTICATE, challenge);
                    }
                    return Err(response);
                };
                let roles: Vec<&str> = allowed_roles.iter().map(String::as_str).collect();
                if self.accounts.allows(&caller, &roles) {
                    Ok(caller)
                } else {
                    Err(error_response(
                        StatusCode::FORBIDDEN,
                        "forbidden",
                        "the caller holds none of the service's roles",
                    ))
                }
            }
        }
    }
}

/// The action's answer — `{status, content, document}` — as the response:
/// the content the flow wrote under its status, else the document.
fn respond(answer: Value) -> Response {
    let status = answer
        .get("status")
        .and_then(Value::as_u64)
        .and_then(|status| u16::try_from(status).ok())
        .and_then(|status| StatusCode::from_u16(status).ok())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    match answer.get("content").and_then(Value::as_str) {
        Some(content) => (status, content.to_string()).into_response(),
        None => (
            status,
            axum::Json(answer.get("document").cloned().unwrap_or(Value::Null)),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt as _;
    use mxrs_runtime::{SecurityPolicy, Store, StoreSchema};
    use tower::ServiceExt as _;

    /// One account, `ana`/`secret`, holding `Sales.User`.
    struct Accounts;

    impl RestAccounts for Accounts {
        fn sign_in(&self, user: &str, password: &str) -> Option<SecurityContext> {
            (user == "ana" && password == "secret").then(|| SecurityContext {
                user: Some("ana".into()),
                ..SecurityContext::default()
            })
        }

        fn anonymous(&self) -> SecurityContext {
            SecurityContext::default()
        }

        fn allows(&self, caller: &SecurityContext, roles: &[&str]) -> bool {
            caller.user.as_deref() == Some("ana") && roles.contains(&"Sales.User")
        }
    }

    fn router() -> Router {
        let mut runtime = Runtime::new(
            Store::new(StoreSchema::default()),
            SecurityPolicy::default(),
        );
        // The action answers what it was given and who asked, under the
        // status the request names.
        runtime.register_action(
            "show",
            |_store: &mut Store, arguments: &Value| -> mxrs_runtime::Result<Value> {
                Ok(json!({
                    "status": arguments["query"]["status"].as_str().map_or(200, |status| status.parse::<u64>().unwrap()),
                    "content": null,
                    "document": arguments,
                }))
            },
        );
        let route = |method: Method, path: &str, authentication| RestRoute {
            method,
            path: path.into(),
            action: "show".into(),
            authentication,
        };
        let basic = RestAuthentication::Basic {
            realm: "Orders".into(),
            allowed_roles: vec!["Sales.User".into()],
        };
        crate::RuntimeHttp::new(runtime, tempfile::tempdir().unwrap().path())
            .with_published_rest(
                vec![
                    route(
                        Method::GET,
                        "/rest/orders/v1/order/{id}",
                        RestAuthentication::Public,
                    ),
                    route(Method::POST, "/rest/orders/v1/order", basic.clone()),
                    route(Method::DELETE, "/rest/orders/v1/order/{id}", basic),
                    route(
                        Method::PUT,
                        "/rest/orders/v1/order/{id}",
                        RestAuthentication::Unsupported("the authentication type(s) custom".into()),
                    ),
                ],
                Arc::new(Accounts),
            )
            .router_without_files()
    }

    async fn call(request: Request<Body>) -> (StatusCode, Value, HeaderMap) {
        let response = router().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&body).unwrap_or(Value::Null),
            headers,
        )
    }

    /// A route answers its method at its path with what the request
    /// carries; the status is the action's.
    #[tokio::test]
    async fn an_operation_answers_its_route() {
        let (status, body, _) = call(
            Request::get("/rest/orders/v1/order/7?expand=true&status=201")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body["path"]["id"], "7");
        assert_eq!(body["query"]["expand"], "true");
        assert_eq!(
            body["uri"],
            "/rest/orders/v1/order/7?expand=true&status=201"
        );
        let (status, _, _) = call(
            Request::patch("/rest/orders/v1/order/7")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    }

    /// A service that asks for Basic signs its caller in, challenging one
    /// without credentials and refusing one without its roles; one asking
    /// for what the runtime does not offer is not served at all.
    #[tokio::test]
    async fn an_operation_signs_its_caller_in_as_its_service_asks() {
        let (status, _, headers) = call(
            Request::post("/rest/orders/v1/order")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(headers[WWW_AUTHENTICATE], "Basic realm=\"Orders\"");
        let (status, _, _) = call(
            Request::post("/rest/orders/v1/order")
                .header(AUTHORIZATION, "Basic YW5hOndyb25n")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, body, _) = call(
            Request::post("/rest/orders/v1/order")
                .header(AUTHORIZATION, "Basic YW5hOnNlY3JldA==")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"note":"rush"}"#))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["body"]["note"], "rush");
        let (status, _, _) = call(
            Request::put("/rest/orders/v1/order/7")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    }
}
