//! Published REST services: the resources a module publishes over HTTP,
//! each operation the microflow it calls and how the request binds to it.
//!
//! A service is stated as Studio Pro stores one. A request reaches an
//! operation at the service's path, then its resource's name, then the
//! operation's own path: `api/v1` + `orders` + `{id}` is
//! `/api/v1/orders/{id}`.

use crate::{ExportLevel, NativeDocument, NativeValue};

/// The HTTP method an operation answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RestMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
}

impl RestMethod {
    /// The name the model stores: `Get`, `Post`, …
    pub fn stored(self) -> &'static str {
        match self {
            Self::Get => "Get",
            Self::Post => "Post",
            Self::Put => "Put",
            Self::Patch => "Patch",
            Self::Delete => "Delete",
            Self::Head => "Head",
            Self::Options => "Options",
        }
    }

    pub fn from_stored(value: &str) -> Option<Self> {
        Some(match value {
            "Get" => Self::Get,
            "Post" => Self::Post,
            "Put" => Self::Put,
            "Patch" => Self::Patch,
            "Delete" => Self::Delete,
            "Head" => Self::Head,
            "Options" => Self::Options,
            _ => return None,
        })
    }

    /// The method as HTTP spells it: `GET`, `POST`, …
    pub fn as_http(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
        }
    }
}

/// Where a request carries a parameter's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RestParameterSource {
    /// A `{name}` segment of the operation's path.
    Path,
    /// The query string.
    Query,
    /// The request body.
    Body,
    /// A request header.
    Header,
}

impl RestParameterSource {
    pub fn stored(self) -> &'static str {
        match self {
            Self::Path => "Path",
            Self::Query => "Query",
            Self::Body => "Body",
            Self::Header => "Header",
        }
    }

    pub fn from_stored(value: &str) -> Option<Self> {
        Some(match value {
            "Path" => Self::Path,
            "Query" => Self::Query,
            "Body" => Self::Body,
            "Header" => Self::Header,
            _ => return None,
        })
    }
}

/// The type of a parameter's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestParameterType {
    String,
    Integer,
    Long,
    Decimal,
    Boolean,
    DateTime,
    /// A value of the enumeration, by its qualified name.
    Enumeration(String),
    /// An object of the entity, by its qualified name: a body an import
    /// mapping reads.
    Object(String),
    /// A file document's contents.
    Binary,
}

impl RestParameterType {
    /// The data type document the model stores for it.
    pub fn document(&self) -> NativeDocument {
        match self {
            Self::String => NativeDocument::new("DataTypes$StringType"),
            Self::Integer => NativeDocument::new("DataTypes$IntegerType"),
            Self::Long => NativeDocument::new("DataTypes$LongType"),
            Self::Decimal => NativeDocument::new("DataTypes$DecimalType"),
            Self::Boolean => NativeDocument::new("DataTypes$BooleanType"),
            Self::DateTime => NativeDocument::new("DataTypes$DateTimeType"),
            Self::Binary => NativeDocument::new("DataTypes$BinaryType"),
            Self::Enumeration(enumeration) => NativeDocument::new("DataTypes$EnumerationType")
                .with("Enumeration", enumeration.as_str()),
            Self::Object(entity) => {
                NativeDocument::new("DataTypes$ObjectType").with("Entity", entity.as_str())
            }
        }
    }

    fn from_document(document: &NativeDocument) -> Option<Self> {
        Some(match document.ty.as_str() {
            "DataTypes$StringType" => Self::String,
            "DataTypes$IntegerType" => Self::Integer,
            "DataTypes$LongType" => Self::Long,
            "DataTypes$DecimalType" => Self::Decimal,
            "DataTypes$BooleanType" => Self::Boolean,
            "DataTypes$DateTimeType" => Self::DateTime,
            "DataTypes$BinaryType" => Self::Binary,
            "DataTypes$EnumerationType" => {
                Self::Enumeration(document.text("Enumeration")?.to_string())
            }
            "DataTypes$ObjectType" => Self::Object(document.text("Entity")?.to_string()),
            _ => return None,
        })
    }
}

/// One parameter of an operation: where the request carries it and the
/// microflow parameter it binds to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestOperationParameter {
    /// The name the request carries it under.
    pub name: String,
    pub source: RestParameterSource,
    pub ty: RestParameterType,
    /// The microflow parameter it binds to, by its own name.
    pub microflow_parameter: String,
    pub description: String,
}

impl RestOperationParameter {
    /// A parameter bound to the microflow parameter of its own name.
    pub fn new(
        name: impl Into<String>,
        source: RestParameterSource,
        ty: RestParameterType,
    ) -> Self {
        let name = name.into();
        Self {
            microflow_parameter: name.clone(),
            name,
            source,
            ty,
            description: String::new(),
        }
    }

    pub fn path(name: impl Into<String>, ty: RestParameterType) -> Self {
        Self::new(name, RestParameterSource::Path, ty)
    }

    pub fn query(name: impl Into<String>, ty: RestParameterType) -> Self {
        Self::new(name, RestParameterSource::Query, ty)
    }

    pub fn body(name: impl Into<String>, ty: RestParameterType) -> Self {
        Self::new(name, RestParameterSource::Body, ty)
    }

    pub fn header(name: impl Into<String>, ty: RestParameterType) -> Self {
        Self::new(name, RestParameterSource::Header, ty)
    }

    /// Binds it to the microflow parameter `name` instead of its own.
    pub fn bound_to(mut self, name: impl Into<String>) -> Self {
        self.microflow_parameter = name.into();
        self
    }

    pub fn description(mut self, value: impl Into<String>) -> Self {
        self.description = value.into();
        self
    }
}

/// What an operation does with the objects an import mapping makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestCommit {
    No,
    Yes,
    YesWithoutEvents,
}

impl RestCommit {
    fn stored(self) -> &'static str {
        match self {
            Self::No => "No",
            Self::Yes => "Yes",
            Self::YesWithoutEvents => "YesWithoutEvents",
        }
    }

    fn from_stored(value: &str) -> Option<Self> {
        Some(match value {
            "No" => Self::No,
            "Yes" => Self::Yes,
            "YesWithoutEvents" => Self::YesWithoutEvents,
            _ => return None,
        })
    }
}

/// One operation of a resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestOperationDecl {
    pub method: RestMethod,
    /// Its path under the resource, without slashes at either end; empty
    /// for the resource's own path.
    pub path: String,
    /// The microflow it calls, by its qualified name.
    pub microflow: String,
    pub summary: String,
    pub documentation: String,
    pub parameters: Vec<RestOperationParameter>,
    /// The export mapping it answers through, by its qualified name; empty
    /// for none.
    pub export_mapping: String,
    /// The import mapping its body is read with; empty for none.
    pub import_mapping: String,
    pub commit: RestCommit,
    pub deprecated: bool,
}

impl RestOperationDecl {
    pub fn new(method: RestMethod, path: impl Into<String>, microflow: impl Into<String>) -> Self {
        Self {
            method,
            path: path.into(),
            microflow: microflow.into(),
            summary: String::new(),
            documentation: String::new(),
            parameters: Vec::new(),
            export_mapping: String::new(),
            import_mapping: String::new(),
            commit: RestCommit::No,
            deprecated: false,
        }
    }

    /// Whether its path ends in a parameter: it addresses one object of the
    /// resource, where another addresses the collection.
    pub fn addresses_one(&self) -> bool {
        self.path
            .trim_matches('/')
            .rsplit('/')
            .next()
            .is_some_and(|segment| segment.starts_with('{') && segment.ends_with('}'))
    }

    fn document(&self) -> NativeDocument {
        let parameters = self
            .parameters
            .iter()
            .map(|parameter| {
                NativeValue::Document(
                    NativeDocument::new("Rest$RestOperationParameter")
                        .with("Description", parameter.description.as_str())
                        .with(
                            "MicroflowParameter",
                            format!("{}.{}", self.microflow, parameter.microflow_parameter),
                        )
                        .with("Name", parameter.name.as_str())
                        .with("ParameterType", parameter.source.stored())
                        .with("Type", parameter.ty.document()),
                )
            })
            .collect();
        NativeDocument::new("Rest$PublishedRestServiceOperation")
            .with("Commit", self.commit.stored())
            .with("Deprecated", self.deprecated)
            .with("Documentation", self.documentation.as_str())
            .with("ExportMapping", self.export_mapping.as_str())
            .with("HttpMethod", self.method.stored())
            .with("ImportMapping", self.import_mapping.as_str())
            .with("Microflow", self.microflow.as_str())
            .with("ObjectHandlingBackup", "Create")
            .with("Parameters", NativeValue::List(3, parameters))
            .with("Path", self.path.as_str())
            .with("Summary", self.summary.as_str())
    }

    fn from_document(document: &NativeDocument) -> Option<Self> {
        let microflow = document.text("Microflow")?;
        let mut operation = Self::new(
            RestMethod::from_stored(document.text("HttpMethod")?)?,
            document.text("Path")?,
            microflow,
        );
        operation.summary = document.text("Summary")?.to_string();
        operation.documentation = document.text("Documentation")?.to_string();
        operation.export_mapping = document.text("ExportMapping")?.to_string();
        operation.import_mapping = document.text("ImportMapping")?.to_string();
        operation.commit = RestCommit::from_stored(document.text("Commit")?)?;
        operation.deprecated = matches!(document.get("Deprecated")?, NativeValue::Bool(true));
        for parameter in documents(document.get("Parameters")?, 3)? {
            let NativeValue::Document(ty) = parameter.get("Type")? else {
                return None;
            };
            let bound = parameter
                .text("MicroflowParameter")?
                .strip_prefix(microflow)?
                .strip_prefix('.')?;
            operation.parameters.push(RestOperationParameter {
                name: parameter.text("Name")?.to_string(),
                source: RestParameterSource::from_stored(parameter.text("ParameterType")?)?,
                ty: RestParameterType::from_document(ty)?,
                microflow_parameter: bound.to_string(),
                description: parameter.text("Description")?.to_string(),
            });
        }
        Some(operation)
    }
}

/// One resource of a service: a name its operations are reached under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestResourceDecl {
    pub name: String,
    pub documentation: String,
    pub operations: Vec<RestOperationDecl>,
}

impl RestResourceDecl {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            documentation: String::new(),
            operations: Vec::new(),
        }
    }
}

/// How a service learns who is calling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestAuthentication {
    /// No authentication: anyone may call it, as the project's guest.
    None,
    /// The authentication types the model lists — `basic`, `session`,
    /// `custom` — and the module roles a caller must hold one of.
    Required {
        types: Vec<String>,
        allowed_roles: Vec<String>,
        /// The microflow `custom` authentication asks; empty otherwise.
        microflow: String,
    },
}

/// A published REST service of a module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedRestServiceDecl {
    pub name: String,
    /// The name its documentation shows, e.g. `Orders API`.
    pub service_name: String,
    pub version: String,
    /// Its path, without slashes at either end: `api/v1`.
    pub path: String,
    pub documentation: String,
    /// The documentation its OpenAPI description publishes.
    pub public_documentation: String,
    pub authentication: RestAuthentication,
    pub resources: Vec<RestResourceDecl>,
    pub excluded: bool,
    pub export_level: ExportLevel,
}

impl PublishedRestServiceDecl {
    /// A service at `path`, open to anyone, named in its documentation as
    /// it is in the model, at version `1.0.0`.
    pub fn new(name: impl Into<String>, path: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            service_name: name.clone(),
            name,
            version: "1.0.0".to_string(),
            path: path.into(),
            documentation: String::new(),
            public_documentation: String::new(),
            authentication: RestAuthentication::None,
            resources: Vec::new(),
            excluded: false,
            export_level: ExportLevel::Hidden,
        }
    }

    /// The path a request reaches `operation` of `resource` at:
    /// `/api/v1/orders/{id}`.
    pub fn route(&self, resource: &RestResourceDecl, operation: &RestOperationDecl) -> String {
        let path = [
            self.path.as_str(),
            resource.name.as_str(),
            operation.path.as_str(),
        ]
        .into_iter()
        .map(|segment| segment.trim_matches('/'))
        .filter(|segment| !segment.is_empty())
        .fold(String::new(), |mut path, segment| {
            path.push('/');
            path.push_str(segment);
            path
        });
        if path.is_empty() {
            "/".to_string()
        } else {
            path
        }
    }

    /// The document the model stores for the service, its fields in the
    /// order Studio Pro stores them.
    pub fn document(&self) -> NativeDocument {
        let (types, roles, microflow) = match &self.authentication {
            RestAuthentication::None => (Vec::new(), Vec::new(), ""),
            RestAuthentication::Required {
                types,
                allowed_roles,
                microflow,
            } => (types.clone(), allowed_roles.clone(), microflow.as_str()),
        };
        let texts = |items: Vec<String>| {
            NativeValue::List(1, items.into_iter().map(NativeValue::Text).collect())
        };
        let resources = self
            .resources
            .iter()
            .map(|resource| {
                NativeValue::Document(
                    NativeDocument::new("Rest$PublishedRestServiceResource")
                        .with("Documentation", resource.documentation.as_str())
                        .with("Name", resource.name.as_str())
                        .with(
                            "Operations",
                            NativeValue::List(
                                2,
                                resource
                                    .operations
                                    .iter()
                                    .map(|operation| NativeValue::Document(operation.document()))
                                    .collect(),
                            ),
                        ),
                )
            })
            .collect();
        NativeDocument::new("Rest$PublishedRestService")
            .with("AllowedRoles", texts(roles))
            .with("AuthenticationMicroflow", microflow)
            .with("AuthenticationTypes", texts(types))
            .with("CorsConfiguration", NativeValue::Null)
            .with("Documentation", self.documentation.as_str())
            .with("Excluded", self.excluded)
            .with(
                "ExportLevel",
                match self.export_level {
                    ExportLevel::Hidden => "Hidden",
                    ExportLevel::Published => "Published",
                },
            )
            .with("Name", self.name.as_str())
            .with("Parameters", NativeValue::List(3, Vec::new()))
            .with("Path", self.path.as_str())
            .with("Resources", NativeValue::List(3, resources))
            .with("ServiceName", self.service_name.as_str())
            .with("Version", self.version.as_str())
            .with("PublicDocumentation", self.public_documentation.as_str())
    }

    /// The declaration a stored service is, when [`Self::document`] states
    /// that document again.
    pub fn from_document(document: &NativeDocument) -> Option<Self> {
        Self::read(document).filter(|service| service.document().says_the_same(document))
    }

    /// What a stored service says, of everything the declaration states —
    /// what a runtime serving it needs — whether or not the declaration
    /// states the document again.
    pub fn read(document: &NativeDocument) -> Option<Self> {
        if document.ty != "Rest$PublishedRestService" {
            return None;
        }
        let texts = |value: &NativeValue| -> Option<Vec<String>> {
            match value {
                NativeValue::List(1, items) => items
                    .iter()
                    .map(|item| match item {
                        NativeValue::Text(text) => Some(text.clone()),
                        _ => None,
                    })
                    .collect(),
                _ => None,
            }
        };
        let mut service = Self::new(document.text("Name")?, document.text("Path")?);
        service.service_name = document.text("ServiceName")?.to_string();
        service.version = document.text("Version")?.to_string();
        service.documentation = document.text("Documentation")?.to_string();
        service.public_documentation = document.text("PublicDocumentation")?.to_string();
        service.excluded = matches!(document.get("Excluded")?, NativeValue::Bool(true));
        service.export_level = match document.text("ExportLevel")? {
            "Hidden" => ExportLevel::Hidden,
            "Published" => ExportLevel::Published,
            _ => return None,
        };
        let types = texts(document.get("AuthenticationTypes")?)?;
        let allowed_roles = texts(document.get("AllowedRoles")?)?;
        let microflow = document.text("AuthenticationMicroflow")?.to_string();
        service.authentication =
            if types.is_empty() && allowed_roles.is_empty() && microflow.is_empty() {
                RestAuthentication::None
            } else {
                RestAuthentication::Required {
                    types,
                    allowed_roles,
                    microflow,
                }
            };
        for resource in documents(document.get("Resources")?, 3)? {
            let mut declared = RestResourceDecl::new(resource.text("Name")?);
            declared.documentation = resource.text("Documentation")?.to_string();
            for operation in documents(resource.get("Operations")?, 2)? {
                declared
                    .operations
                    .push(RestOperationDecl::from_document(operation)?);
            }
            service.resources.push(declared);
        }
        Some(service)
    }
}

/// The documents of a stored list with `marker`.
fn documents(value: &NativeValue, marker: i32) -> Option<Vec<&NativeDocument>> {
    match value {
        NativeValue::List(stored, items) if *stored == marker => items
            .iter()
            .map(|item| match item {
                NativeValue::Document(document) => Some(document),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn orders() -> PublishedRestServiceDecl {
        let mut service = PublishedRestServiceDecl::new("OrdersApi", "api/v1");
        service.service_name = "Orders API".into();
        service.authentication = RestAuthentication::Required {
            types: vec!["basic".into()],
            allowed_roles: vec!["Sales.User".into()],
            microflow: String::new(),
        };
        let mut resource = RestResourceDecl::new("orders");
        let mut show = RestOperationDecl::new(RestMethod::Get, "{id}", "Sales.MF_Order_Show");
        show.parameters.push(RestOperationParameter::path(
            "id",
            RestParameterType::Integer,
        ));
        show.parameters.push(
            RestOperationParameter::query("expand", RestParameterType::Boolean).bound_to("Expand"),
        );
        show.export_mapping = "Sales.EM_Order".into();
        resource.operations.push(show);
        resource.operations.push(RestOperationDecl::new(
            RestMethod::Post,
            "",
            "Sales.MF_Order_Create",
        ));
        service.resources.push(resource);
        service
    }

    /// A parameter names the microflow parameter it binds by the
    /// microflow's qualified name, and the declaration reads back.
    #[test]
    fn a_service_is_stored_as_studio_pro_stores_one() {
        let service = orders();
        let document = service.document();
        let parameter = document
            .at("Resources[0].Operations[0].Parameters[1]")
            .unwrap();
        assert_eq!(
            parameter.text("MicroflowParameter"),
            Some("Sales.MF_Order_Show.Expand")
        );
        assert_eq!(parameter.text("ParameterType"), Some("Query"));
        assert_eq!(
            PublishedRestServiceDecl::from_document(&document),
            Some(service)
        );
    }

    /// The service path, the resource and the operation's path make the
    /// route, whatever slashes each carries.
    #[test]
    fn a_route_joins_service_resource_and_operation() {
        let service = orders();
        let resource = &service.resources[0];
        assert_eq!(
            service.route(resource, &resource.operations[0]),
            "/api/v1/orders/{id}"
        );
        assert_eq!(
            service.route(resource, &resource.operations[1]),
            "/api/v1/orders"
        );
        assert!(resource.operations[0].addresses_one());
        assert!(!resource.operations[1].addresses_one());
    }

    /// What the declaration cannot state — a CORS configuration here — is
    /// not read as a declaration.
    #[test]
    fn a_service_it_cannot_state_is_not_declared() {
        let mut document = orders().document();
        document.set(
            "CorsConfiguration",
            NativeDocument::new("Rest$CorsConfiguration"),
        );
        assert_eq!(PublishedRestServiceDecl::from_document(&document), None);
    }
}
