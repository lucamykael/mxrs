//! The implicit `System.HttpRequest` / `System.HttpResponse` parameters a
//! published REST operation's microflow declares.
//!
//! Mendix supplies these itself: the operation's own parameter list never
//! mentions them, so a boundary that binds only the declared parameters leaves
//! the microflow short an argument and the call fails before it starts.
//!
//! They are not decoration. A flow changes its response object to choose the
//! status and write the body — that is how an operation documented as answering
//! `404` answers `404` — so binding the objects and reading back what the flow
//! wrote are two halves of one thing, and neither is useful alone.
//!
//! What is *not* carried: headers. `System.HttpHeader` exists in the store, but
//! no header object is created or associated, so a flow that retrieves
//! `System.HttpHeaders` finds none and a content type the flow sets through a
//! header is not honoured.

use mxrs_runtime::{RuntimeError, Store};

use crate::value::{FlowValue, ObjectRef, Variables};

/// Which implicit parameters a microflow declares, by the variable names it
/// declares them under.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HttpObjects {
    /// The request parameter's name and the URI its object carries. The URI
    /// belongs to the declaration because nothing else reads it: an operation
    /// whose microflow asks for no request object has no use for one.
    request: Option<(String, String)>,
    /// The request's body, as text, which the request object carries.
    content: Option<String>,
    response: Option<String>,
}

impl HttpObjects {
    /// Nothing implicit to bind — the ordinary case.
    pub fn none() -> Self {
        Self::default()
    }

    /// The microflow takes its request object under `parameter`, describing the
    /// request at `uri`.
    pub fn with_request(mut self, parameter: impl Into<String>, uri: impl Into<String>) -> Self {
        self.request = Some((parameter.into(), uri.into()));
        self
    }

    /// The request's body, as text: what its request object's `Content`
    /// holds.
    pub fn with_content(mut self, content: impl Into<String>) -> Self {
        self.content = Some(content.into());
        self
    }

    /// The microflow takes its response object under `parameter`.
    pub fn with_response(mut self, parameter: impl Into<String>) -> Self {
        self.response = Some(parameter.into());
        self
    }

    pub fn is_empty(&self) -> bool {
        self.request.is_none() && self.response.is_none()
    }

    /// Creates the objects the microflow declares and binds them as arguments,
    /// answering the binding needed to read the response back afterwards.
    ///
    /// The response object starts at the System module's own defaults — `200`,
    /// `OK`, empty content — so a flow that never touches it leaves the
    /// boundary's own answer intact.
    pub fn bind(
        &self,
        store: &mut Store,
        arguments: &mut Variables,
    ) -> Result<HttpBinding, RuntimeError> {
        let mut binding = HttpBinding::default();
        if let Some((parameter, uri)) = &self.request {
            let request = store.create("System.HttpRequest")?;
            store.set_member(
                &request.entity,
                &request.id,
                "Uri",
                serde_json::Value::String(uri.clone()),
            )?;
            if let Some(content) = &self.content {
                store.set_member(
                    &request.entity,
                    &request.id,
                    "Content",
                    serde_json::Value::String(content.clone()),
                )?;
            }
            arguments.insert(
                parameter.clone(),
                FlowValue::Object(ObjectRef {
                    entity: request.entity,
                    id: request.id,
                }),
            );
        }
        if let Some(parameter) = &self.response {
            let response = store.create("System.HttpResponse")?;
            let reference = ObjectRef {
                entity: response.entity,
                id: response.id,
            };
            arguments.insert(parameter.clone(), FlowValue::Object(reference.clone()));
            binding.response = Some(reference);
        }
        Ok(binding)
    }
}

/// The objects a call bound, so the boundary can read back what the flow wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HttpBinding {
    response: Option<ObjectRef>,
}

impl HttpBinding {
    /// What the flow left on its response object, or `None` when it had none to
    /// leave anything on.
    ///
    /// A flow that never touched the object answers the System defaults, which
    /// say exactly what the boundary would have said on its own — so there is
    /// no need to tell "untouched" from "set to 200".
    pub fn answer(&self, store: &Store) -> Option<HttpAnswer> {
        let reference = self.response.as_ref()?;
        let object = store
            .find(&reference.entity, &reference.id)
            .ok()
            .flatten()?;
        let text = |member: &str| {
            object
                .members
                .get(member)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string()
        };
        Some(HttpAnswer {
            // A status outside the u16 range is not a status; the boundary
            // turns an unusable one into a server error rather than guessing.
            status: object
                .members
                .get("StatusCode")
                .and_then(|value| value.as_i64())
                .and_then(|status| u16::try_from(status).ok())
                .unwrap_or(0),
            reason: text("ReasonPhrase"),
            content: text("Content"),
        })
    }
}

/// What a flow chose for its operation's HTTP response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpAnswer {
    /// `System.HttpResponse.StatusCode`, or `0` when it is not a usable status.
    pub status: u16,
    /// `System.HttpResponse.ReasonPhrase`. HTTP/2 has no reason phrase, so this
    /// is carried for the caller's own use rather than put on the wire.
    pub reason: String,
    /// `System.HttpMessage.Content` — empty when the flow wrote no body of its
    /// own and the operation's own document should answer instead.
    pub content: String,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;
    use mxrs_runtime::StoreSchema;

    /// The same four entities `mxrs_runtime_boot` registers, so this crate's
    /// tests do not depend on the boot crate.
    fn store() -> Store {
        Store::new(
            StoreSchema::default()
                .entity(
                    "System.HttpRequest",
                    BTreeMap::from([
                        ("HttpVersion".to_string(), json!("HTTP/1.1")),
                        ("Content".to_string(), json!("")),
                        ("Uri".to_string(), json!("")),
                    ]),
                    true,
                )
                .entity(
                    "System.HttpResponse",
                    BTreeMap::from([
                        ("HttpVersion".to_string(), json!("HTTP/1.1")),
                        ("Content".to_string(), json!("")),
                        ("StatusCode".to_string(), json!(200)),
                        ("ReasonPhrase".to_string(), json!("OK")),
                    ]),
                    true,
                ),
        )
    }

    #[test]
    fn nothing_is_bound_when_the_microflow_declares_nothing() {
        let mut store = store();
        let mut arguments = Variables::new();
        let binding = HttpObjects::none()
            .bind(&mut store, &mut arguments)
            .unwrap();

        assert!(HttpObjects::none().is_empty());
        assert!(arguments.is_empty());
        assert_eq!(binding.answer(&store), None);
    }

    #[test]
    fn a_declared_request_is_bound_carrying_the_uri() {
        let mut store = store();
        let mut arguments = Variables::new();
        HttpObjects::none()
            .with_request("HttpRequest", "/api/v1/orders?trace=1")
            .bind(&mut store, &mut arguments)
            .unwrap();

        let FlowValue::Object(reference) = &arguments["HttpRequest"] else {
            panic!("the request parameter is bound to an object");
        };
        let request = store
            .find(&reference.entity, &reference.id)
            .unwrap()
            .unwrap();
        assert_eq!(request.members["Uri"], json!("/api/v1/orders?trace=1"));
        assert_eq!(request.members["HttpVersion"], json!("HTTP/1.1"));
    }

    /// A flow that never touches its response object leaves the System
    /// defaults, which say what the boundary would have said anyway.
    #[test]
    fn an_untouched_response_answers_the_system_defaults() {
        let mut store = store();
        let mut arguments = Variables::new();
        let binding = HttpObjects::none()
            .with_response("HttpResponse")
            .bind(&mut store, &mut arguments)
            .unwrap();

        assert_eq!(
            binding.answer(&store),
            Some(HttpAnswer {
                status: 200,
                reason: "OK".to_string(),
                content: String::new(),
            })
        );
    }

    /// The path a published operation documented as answering `404` actually
    /// takes: the flow changes its response object, and the boundary reads it.
    #[test]
    fn a_flow_that_changed_its_response_is_read_back() {
        let mut store = store();
        let mut arguments = Variables::new();
        let binding = HttpObjects::none()
            .with_response("HttpResponse")
            .bind(&mut store, &mut arguments)
            .unwrap();
        let FlowValue::Object(reference) = &arguments["HttpResponse"] else {
            panic!("the response parameter is bound to an object");
        };
        for (member, value) in [
            ("StatusCode", json!(404)),
            ("ReasonPhrase", json!("Not Found")),
            ("Content", json!("Invalid or missing BuildingID")),
        ] {
            store
                .set_member(&reference.entity, &reference.id, member, value)
                .unwrap();
        }

        assert_eq!(
            binding.answer(&store),
            Some(HttpAnswer {
                status: 404,
                reason: "Not Found".to_string(),
                content: "Invalid or missing BuildingID".to_string(),
            })
        );
    }

    #[test]
    fn a_status_outside_the_http_range_is_not_a_status() {
        let mut store = store();
        let mut arguments = Variables::new();
        let binding = HttpObjects::none()
            .with_response("HttpResponse")
            .bind(&mut store, &mut arguments)
            .unwrap();
        let FlowValue::Object(reference) = &arguments["HttpResponse"] else {
            panic!("the response parameter is bound to an object");
        };
        store
            .set_member(
                &reference.entity,
                &reference.id,
                "StatusCode",
                json!(70_000),
            )
            .unwrap();

        assert_eq!(binding.answer(&store).map(|answer| answer.status), Some(0));
    }

    #[test]
    fn an_entity_the_store_does_not_have_fails_rather_than_binding_nothing() {
        let mut store = Store::new(StoreSchema::default());
        let mut arguments = Variables::new();
        let error = HttpObjects::none()
            .with_response("HttpResponse")
            .bind(&mut store, &mut arguments)
            .unwrap_err();

        assert_eq!(
            error,
            RuntimeError::UnknownEntity("System.HttpResponse".to_string())
        );
        assert!(arguments.is_empty());
    }
}
