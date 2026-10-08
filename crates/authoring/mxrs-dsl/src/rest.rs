//! A published REST service, as a module declares it:
//!
//! ```
//! # use mxrs_dsl::{ModuleBuilder, PublishedRestServiceBuilder};
//! # use mxrs_ir::RestParameterType;
//! # let mut module = ModuleBuilder::new("Sales");
//! let mut service = PublishedRestServiceBuilder::new("OrdersApi", "api/v1");
//! service
//!     .basic_authentication(&["Sales.User"])
//!     .resource("orders", |orders| {
//!         orders.get("{id}", "Sales.MF_Order_Show", |show| {
//!             show.summary("One order")
//!                 .path_parameter("id", RestParameterType::Integer)
//!                 .export_mapping("Sales.EM_Order");
//!         });
//!     });
//! module.published_rest_service(service);
//! ```

use mxrs_ir::{
    ExportLevel, PublishedRestServiceDecl, RestAuthentication, RestCommit, RestMethod,
    RestOperationDecl, RestOperationParameter, RestParameterType, RestResourceDecl,
};

/// A service: where it is published, who may call it, and its resources.
pub struct PublishedRestServiceBuilder {
    decl: PublishedRestServiceDecl,
}

impl PublishedRestServiceBuilder {
    /// A service at `path`, open to anyone, at version `1.0.0`.
    pub fn new(name: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            decl: PublishedRestServiceDecl::new(name, path),
        }
    }

    /// The name its documentation shows; the service's own by default.
    pub fn service_name(&mut self, value: impl Into<String>) -> &mut Self {
        self.decl.service_name = value.into();
        self
    }

    pub fn version(&mut self, value: impl Into<String>) -> &mut Self {
        self.decl.version = value.into();
        self
    }

    pub fn documentation(&mut self, value: impl Into<String>) -> &mut Self {
        self.decl.documentation = value.into();
        self
    }

    /// The documentation its OpenAPI description publishes.
    pub fn public_documentation(&mut self, value: impl Into<String>) -> &mut Self {
        self.decl.public_documentation = value.into();
        self
    }

    /// HTTP Basic authentication; a caller must hold one of `roles`, the
    /// module roles by their qualified names.
    pub fn basic_authentication(&mut self, roles: &[&str]) -> &mut Self {
        self.authentication(&["basic"], roles, "")
    }

    /// The authentication `types` the model lists — `basic`, `session`,
    /// `custom` — the module roles a caller must hold one of, and the
    /// microflow `custom` authentication asks (empty for none).
    pub fn authentication(
        &mut self,
        types: &[&str],
        roles: &[&str],
        microflow: impl Into<String>,
    ) -> &mut Self {
        self.decl.authentication = RestAuthentication::Required {
            types: types.iter().map(ToString::to_string).collect(),
            allowed_roles: roles.iter().map(ToString::to_string).collect(),
            microflow: microflow.into(),
        };
        self
    }

    pub fn excluded(&mut self, value: bool) -> &mut Self {
        self.decl.excluded = value;
        self
    }

    pub fn export_level(&mut self, value: ExportLevel) -> &mut Self {
        self.decl.export_level = value;
        self
    }

    /// A resource: the name its operations are reached under. Stated again,
    /// a resource gains the operations stated with it.
    pub fn resource(
        &mut self,
        name: impl Into<String>,
        configure: impl FnOnce(&mut RestResourceBuilder),
    ) -> &mut Self {
        let mut builder = RestResourceBuilder::new(name);
        configure(&mut builder);
        self.push_resource(builder.into_decl());
        self
    }

    /// A resource stated whole, joining the one of its name when the service
    /// has it already; answers its position among the service's resources.
    pub fn push_resource(&mut self, resource: RestResourceDecl) -> usize {
        match self
            .decl
            .resources
            .iter()
            .position(|existing| existing.name == resource.name)
        {
            Some(position) => {
                let existing = &mut self.decl.resources[position];
                if existing.documentation.is_empty() {
                    existing.documentation = resource.documentation;
                }
                existing.operations.extend(resource.operations);
                position
            }
            None => {
                self.decl.resources.push(resource);
                self.decl.resources.len() - 1
            }
        }
    }

    /// The service as declared so far.
    pub fn decl(&self) -> &PublishedRestServiceDecl {
        &self.decl
    }

    pub fn into_decl(self) -> PublishedRestServiceDecl {
        self.decl
    }
}

impl From<PublishedRestServiceBuilder> for PublishedRestServiceDecl {
    fn from(builder: PublishedRestServiceBuilder) -> Self {
        builder.into_decl()
    }
}

/// One resource of a service.
pub struct RestResourceBuilder {
    decl: RestResourceDecl,
}

impl RestResourceBuilder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            decl: RestResourceDecl::new(name),
        }
    }

    pub fn documentation(&mut self, value: impl Into<String>) -> &mut Self {
        self.decl.documentation = value.into();
        self
    }

    /// An operation: the `method` it answers at `path` under the resource,
    /// and the `microflow`, by its qualified name, it calls.
    pub fn operation(
        &mut self,
        method: RestMethod,
        path: impl Into<String>,
        microflow: impl Into<String>,
        configure: impl FnOnce(&mut RestOperationBuilder),
    ) -> &mut Self {
        let mut builder = RestOperationBuilder::new(method, path, microflow);
        configure(&mut builder);
        self.decl.operations.push(builder.into_decl());
        self
    }

    pub fn get(
        &mut self,
        path: impl Into<String>,
        microflow: impl Into<String>,
        configure: impl FnOnce(&mut RestOperationBuilder),
    ) -> &mut Self {
        self.operation(RestMethod::Get, path, microflow, configure)
    }

    pub fn post(
        &mut self,
        path: impl Into<String>,
        microflow: impl Into<String>,
        configure: impl FnOnce(&mut RestOperationBuilder),
    ) -> &mut Self {
        self.operation(RestMethod::Post, path, microflow, configure)
    }

    pub fn put(
        &mut self,
        path: impl Into<String>,
        microflow: impl Into<String>,
        configure: impl FnOnce(&mut RestOperationBuilder),
    ) -> &mut Self {
        self.operation(RestMethod::Put, path, microflow, configure)
    }

    pub fn patch(
        &mut self,
        path: impl Into<String>,
        microflow: impl Into<String>,
        configure: impl FnOnce(&mut RestOperationBuilder),
    ) -> &mut Self {
        self.operation(RestMethod::Patch, path, microflow, configure)
    }

    pub fn delete(
        &mut self,
        path: impl Into<String>,
        microflow: impl Into<String>,
        configure: impl FnOnce(&mut RestOperationBuilder),
    ) -> &mut Self {
        self.operation(RestMethod::Delete, path, microflow, configure)
    }

    pub fn into_decl(self) -> RestResourceDecl {
        self.decl
    }
}

/// One operation of a resource.
pub struct RestOperationBuilder {
    decl: RestOperationDecl,
}

impl RestOperationBuilder {
    pub fn new(method: RestMethod, path: impl Into<String>, microflow: impl Into<String>) -> Self {
        Self {
            decl: RestOperationDecl::new(method, path, microflow),
        }
    }

    pub fn summary(&mut self, value: impl Into<String>) -> &mut Self {
        self.decl.summary = value.into();
        self
    }

    pub fn documentation(&mut self, value: impl Into<String>) -> &mut Self {
        self.decl.documentation = value.into();
        self
    }

    /// A parameter stated whole: where it comes from, its type, the
    /// microflow parameter it binds and its description.
    pub fn parameter(&mut self, parameter: RestOperationParameter) -> &mut Self {
        self.decl.parameters.push(parameter);
        self
    }

    /// A `{name}` of the path, bound to the microflow parameter `name`.
    pub fn path_parameter(&mut self, name: impl Into<String>, ty: RestParameterType) -> &mut Self {
        self.parameter(RestOperationParameter::path(name, ty))
    }

    /// A query string parameter, bound to the microflow parameter `name`.
    pub fn query_parameter(&mut self, name: impl Into<String>, ty: RestParameterType) -> &mut Self {
        self.parameter(RestOperationParameter::query(name, ty))
    }

    /// The request body, bound to the microflow parameter `name`.
    pub fn body_parameter(&mut self, name: impl Into<String>, ty: RestParameterType) -> &mut Self {
        self.parameter(RestOperationParameter::body(name, ty))
    }

    /// A request header, bound to the microflow parameter `name`.
    pub fn header_parameter(
        &mut self,
        name: impl Into<String>,
        ty: RestParameterType,
    ) -> &mut Self {
        self.parameter(RestOperationParameter::header(name, ty))
    }

    /// The export mapping it answers through, by its qualified name.
    pub fn export_mapping(&mut self, value: impl Into<String>) -> &mut Self {
        self.decl.export_mapping = value.into();
        self
    }

    /// The import mapping its body is read with, by its qualified name.
    pub fn import_mapping(&mut self, value: impl Into<String>) -> &mut Self {
        self.decl.import_mapping = value.into();
        self
    }

    /// Whether the objects its import mapping makes are committed.
    pub fn commit(&mut self, value: RestCommit) -> &mut Self {
        self.decl.commit = value;
        self
    }

    pub fn deprecated(&mut self, value: bool) -> &mut Self {
        self.decl.deprecated = value;
        self
    }

    pub fn into_decl(self) -> RestOperationDecl {
        self.decl
    }
}
