//! A published REST service as one statement: what the model publishes and
//! the handler serving each operation.
//!
//! The service's declaration — its path, who may call it, its resources and
//! operations — is the document the build writes, and the router serves
//! exactly that declaration: an operation's route is its service's path,
//! its resource's name and its own path, and its method is the one it
//! declares. Changing a path or a method is changing the declaration; the
//! router follows.
//!
//! ```
//! use axum::Json;
//! use mxrs::rest::Service;
//! use mxrs::RestParameterType;
//!
//! async fn show() -> Json<&'static str> {
//!     Json("an order")
//! }
//!
//! let service: Service<()> = Service::new("OrdersApi", "api/v1")
//!     .basic_authentication(&["Sales.User"])
//!     .resource("orders", |orders| {
//!         orders.get("{id}", "Sales.MF_Order_Show", show, |operation| {
//!             operation.path_parameter("id", RestParameterType::Integer);
//!         });
//!     });
//! let declared = service.declaration();
//! assert_eq!(
//!     declared.route(&declared.resources[0], &declared.resources[0].operations[0]),
//!     "/api/v1/orders/{id}"
//! );
//! let _router: axum::Router<()> = service.router();
//! ```

use axum::Router;
use axum::handler::Handler;
use axum::routing::{MethodFilter, MethodRouter, on};
use mxrs_dsl::{PublishedRestServiceBuilder, RestOperationBuilder, RestResourceBuilder};
use mxrs_ir::{ExportLevel, PublishedRestServiceDecl, RestMethod};

/// A published REST service and the handlers serving its operations, for
/// an application whose state is `S`.
pub struct Service<S> {
    service: PublishedRestServiceBuilder,
    /// Each resource's handlers, one per operation in the order it
    /// declares them.
    handlers: Vec<Vec<MethodRouter<S>>>,
}

impl<S: Clone + Send + Sync + 'static> Service<S> {
    /// A service at `path`, open to anyone, at version `1.0.0`.
    pub fn new(name: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            service: PublishedRestServiceBuilder::new(name, path),
            handlers: Vec::new(),
        }
    }

    /// The name its documentation shows; the service's own by default.
    pub fn service_name(mut self, value: impl Into<String>) -> Self {
        self.service.service_name(value);
        self
    }

    pub fn version(mut self, value: impl Into<String>) -> Self {
        self.service.version(value);
        self
    }

    pub fn documentation(mut self, value: impl Into<String>) -> Self {
        self.service.documentation(value);
        self
    }

    /// The documentation its OpenAPI description publishes.
    pub fn public_documentation(mut self, value: impl Into<String>) -> Self {
        self.service.public_documentation(value);
        self
    }

    /// HTTP Basic authentication; a caller must hold one of `roles`.
    pub fn basic_authentication(mut self, roles: &[&str]) -> Self {
        self.service.basic_authentication(roles);
        self
    }

    /// The authentication `types` the model lists, the module roles a caller
    /// must hold one of, and the microflow `custom` authentication asks.
    pub fn authentication(
        mut self,
        types: &[&str],
        roles: &[&str],
        microflow: impl Into<String>,
    ) -> Self {
        self.service.authentication(types, roles, microflow);
        self
    }

    pub fn excluded(mut self, value: bool) -> Self {
        self.service.excluded(value);
        self
    }

    pub fn export_level(mut self, value: ExportLevel) -> Self {
        self.service.export_level(value);
        self
    }

    /// A resource and the handlers of its operations.
    pub fn resource(
        mut self,
        name: impl Into<String>,
        configure: impl FnOnce(&mut Resource<S>),
    ) -> Self {
        let mut resource = Resource {
            resource: RestResourceBuilder::new(name),
            handlers: Vec::new(),
        };
        configure(&mut resource);
        self.service.push_resource(resource.resource.into_decl());
        self.handlers.push(resource.handlers);
        self
    }

    /// What the service publishes: the document the build writes.
    pub fn declaration(&self) -> PublishedRestServiceDecl {
        self.service.decl().clone()
    }

    /// The routes the service declares, each served by its handler: an
    /// operation's route is the service's path, its resource's name and
    /// its own path, and it answers the method it declares.
    ///
    /// # Panics
    ///
    /// When two operations answer one method at one route — which the
    /// model cannot publish either.
    pub fn router(self) -> Router<S> {
        let service = self.service.into_decl();
        let mut routes: Vec<(String, MethodRouter<S>)> = Vec::new();
        for (resource, handlers) in service.resources.iter().zip(self.handlers) {
            for (operation, handler) in resource.operations.iter().zip(handlers) {
                let route = service.route(resource, operation);
                match routes.iter_mut().find(|(path, _)| *path == route) {
                    Some((_, existing)) => {
                        *existing = std::mem::take(existing).merge(handler);
                    }
                    None => routes.push((route, handler)),
                }
            }
        }
        routes
            .into_iter()
            .fold(Router::new(), |router, (route, handler)| {
                router.route(&route, handler)
            })
    }
}

/// One resource of a [`Service`], declaring each operation with the handler
/// that serves it.
pub struct Resource<S> {
    resource: RestResourceBuilder,
    handlers: Vec<MethodRouter<S>>,
}

impl<S: Clone + Send + Sync + 'static> Resource<S> {
    pub fn documentation(&mut self, value: impl Into<String>) -> &mut Self {
        self.resource.documentation(value);
        self
    }

    /// An operation: the `method` it answers at `path` under the resource,
    /// the `microflow` the model binds to it, and the `handler` serving it.
    pub fn operation<H, T>(
        &mut self,
        method: RestMethod,
        path: impl Into<String>,
        microflow: impl Into<String>,
        handler: H,
        configure: impl FnOnce(&mut RestOperationBuilder),
    ) -> &mut Self
    where
        H: Handler<T, S>,
        T: 'static,
    {
        self.resource.operation(method, path, microflow, configure);
        self.handlers.push(on(filter(method), handler));
        self
    }

    pub fn get<H, T>(
        &mut self,
        path: impl Into<String>,
        microflow: impl Into<String>,
        handler: H,
        configure: impl FnOnce(&mut RestOperationBuilder),
    ) -> &mut Self
    where
        H: Handler<T, S>,
        T: 'static,
    {
        self.operation(RestMethod::Get, path, microflow, handler, configure)
    }

    pub fn post<H, T>(
        &mut self,
        path: impl Into<String>,
        microflow: impl Into<String>,
        handler: H,
        configure: impl FnOnce(&mut RestOperationBuilder),
    ) -> &mut Self
    where
        H: Handler<T, S>,
        T: 'static,
    {
        self.operation(RestMethod::Post, path, microflow, handler, configure)
    }

    pub fn put<H, T>(
        &mut self,
        path: impl Into<String>,
        microflow: impl Into<String>,
        handler: H,
        configure: impl FnOnce(&mut RestOperationBuilder),
    ) -> &mut Self
    where
        H: Handler<T, S>,
        T: 'static,
    {
        self.operation(RestMethod::Put, path, microflow, handler, configure)
    }

    pub fn patch<H, T>(
        &mut self,
        path: impl Into<String>,
        microflow: impl Into<String>,
        handler: H,
        configure: impl FnOnce(&mut RestOperationBuilder),
    ) -> &mut Self
    where
        H: Handler<T, S>,
        T: 'static,
    {
        self.operation(RestMethod::Patch, path, microflow, handler, configure)
    }

    pub fn delete<H, T>(
        &mut self,
        path: impl Into<String>,
        microflow: impl Into<String>,
        handler: H,
        configure: impl FnOnce(&mut RestOperationBuilder),
    ) -> &mut Self
    where
        H: Handler<T, S>,
        T: 'static,
    {
        self.operation(RestMethod::Delete, path, microflow, handler, configure)
    }
}

fn filter(method: RestMethod) -> MethodFilter {
    match method {
        RestMethod::Get => MethodFilter::GET,
        RestMethod::Post => MethodFilter::POST,
        RestMethod::Put => MethodFilter::PUT,
        RestMethod::Patch => MethodFilter::PATCH,
        RestMethod::Delete => MethodFilter::DELETE,
        RestMethod::Head => MethodFilter::HEAD,
        RestMethod::Options => MethodFilter::OPTIONS,
    }
}
