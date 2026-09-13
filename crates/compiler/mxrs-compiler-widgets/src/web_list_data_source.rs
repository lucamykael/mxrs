//! Resolves the list source used by modern React widgets.
//!
//! Ports `Mxrb::Compiler::WebListDataSource`: XPath, association,
//! microflow, nanoflow, and the deliberately narrow fallback from a simple
//! unconfigured Database Connector SELECT to the mapped entity's XPath
//! source.

use mxrs_bson::{Bson, Document};
use mxrs_compiler_support::{array_docs, get_doc_any, get_str_any};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    XPath,
    Association,
    Microflow,
    Nanoflow,
}

/// A resolved list source. `documents` passed to [`Self::from_documents`]
/// are `(owning module name, raw unit document)` pairs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebListDataSource {
    kind: Option<SourceKind>,
    pub association_path: String,
    pub entity: String,
    pub microflow_name: String,
    pub nanoflow_name: String,
    pub xpath_constraint: String,
}

impl WebListDataSource {
    pub fn from_documents(documents: &[(String, Document)], widget: &Document) -> Self {
        let direct = widget.get_document("DataSource").ok().cloned();
        let xpath = direct
            .as_ref()
            .filter(|source| database_source(source))
            .cloned()
            .or_else(|| nested_first(widget, "CustomWidgets$CustomWidgetXPathSource"));
        let association = direct
            .as_ref()
            .filter(|source| {
                matches!(
                    source.get_str("$Type").ok(),
                    Some("Forms$AssociationSource" | "Forms$ReferenceSetSource")
                )
            })
            .cloned()
            .or_else(|| nested_first(widget, "Forms$AssociationSource"));
        let microflow = direct
            .as_ref()
            .filter(|source| source.get_str("$Type").ok() == Some("Forms$MicroflowSource"))
            .cloned()
            .or_else(|| nested_first(widget, "Forms$MicroflowSource"));
        let nanoflow = direct
            .as_ref()
            .filter(|source| source.get_str("$Type").ok() == Some("Forms$NanoflowSource"))
            .cloned()
            .or_else(|| nested_first(widget, "Forms$NanoflowSource"));

        let microflow_name = microflow
            .as_ref()
            .map(|source| flow_name(source, "Microflow"))
            .unwrap_or_default();
        let nanoflow_name = nanoflow
            .as_ref()
            .map(|source| flow_name(source, "Nanoflow"))
            .unwrap_or_default();
        let flow = qualified_unit(documents, "Microflows$Microflow", &microflow_name);
        let nanoflow_unit = qualified_unit(documents, "Microflows$Nanoflow", &nanoflow_name);

        let entity = xpath
            .as_ref()
            .and_then(|source| source.get_document("EntityRef").ok())
            .and_then(entity_ref_destination)
            .or_else(|| {
                association
                    .as_ref()
                    .and_then(|source| source.get_document("EntityRef").ok())
                    .and_then(entity_ref_destination)
            })
            .or_else(|| flow.and_then(return_entity))
            .or_else(|| nanoflow_unit.and_then(return_entity))
            .unwrap_or_default();
        let association_path = association
            .as_ref()
            .and_then(|source| source.get_document("EntityRef").ok())
            .map(entity_ref_path)
            .unwrap_or_default();
        let xpath_constraint = xpath
            .as_ref()
            .and_then(|source| source.get_str("XPathConstraint").ok())
            .unwrap_or("")
            .to_string();

        let connector_fallback =
            flow.is_some_and(|flow| simple_unconfigured_connector_query(documents, flow, &entity));
        let kind = if xpath.is_some() || connector_fallback {
            Some(SourceKind::XPath)
        } else if association.is_some() && !association_path.is_empty() && !entity.is_empty() {
            Some(SourceKind::Association)
        } else if microflow.is_some() && !microflow_name.is_empty() && flow.is_some() {
            Some(SourceKind::Microflow)
        } else if nanoflow.is_some() && !nanoflow_name.is_empty() && nanoflow_unit.is_some() {
            Some(SourceKind::Nanoflow)
        } else {
            None
        };

        Self {
            kind,
            association_path,
            entity,
            microflow_name,
            nanoflow_name,
            xpath_constraint,
        }
    }

    pub fn supported(&self) -> bool {
        self.kind.is_some()
    }

    pub fn xpath(&self) -> bool {
        self.kind == Some(SourceKind::XPath)
    }

    pub fn association(&self) -> bool {
        self.kind == Some(SourceKind::Association)
    }

    pub fn microflow(&self) -> bool {
        self.kind == Some(SourceKind::Microflow)
    }

    pub fn nanoflow(&self) -> bool {
        self.kind == Some(SourceKind::Nanoflow)
    }
}

fn database_source(source: &Document) -> bool {
    matches!(
        source.get_str("$Type").ok(),
        Some(
            "Forms$DatabaseSource"
                | "Forms$NewListViewDatabaseSource"
                | "Forms$ListViewXPathSource"
                | "Forms$GridXPathSource"
                | "Forms$NewGridDatabaseSource"
                | "CustomWidgets$CustomWidgetXPathSource"
                | "CustomWidgets$CustomWidgetDatabaseSource"
        )
    )
}

fn flow_name(source: &Document, key: &str) -> String {
    source
        .get_document(format!("{key}Settings"))
        .ok()
        .and_then(|settings| settings.get_str(key).ok())
        .filter(|name| !name.is_empty())
        .or_else(|| source.get_str(key).ok())
        .unwrap_or("")
        .to_string()
}

fn qualified_unit<'a>(
    documents: &'a [(String, Document)],
    type_name: &str,
    qualified_name: &str,
) -> Option<&'a Document> {
    documents.iter().find_map(|(module, document)| {
        (document.get_str("$Type").ok() == Some(type_name)
            && format!("{module}.{}", document.get_str("Name").unwrap_or_default())
                == qualified_name)
            .then_some(document)
    })
}

fn return_entity(document: &Document) -> Option<String> {
    get_doc_any(document, &["MicroflowReturnType"])
        .and_then(|return_type| get_str_any(&return_type, &["Entity"]))
        .filter(|entity| !entity.is_empty())
}

fn entity_ref_destination(reference: &Document) -> Option<String> {
    array_docs(reference, &["Steps"])
        .last()
        .and_then(|step| get_str_any(step, &["DestinationEntity"]))
        .or_else(|| get_str_any(reference, &["Entity"]))
        .filter(|entity| !entity.is_empty())
}

fn entity_ref_path(reference: &Document) -> String {
    array_docs(reference, &["Steps"])
        .iter()
        .flat_map(|step| {
            [
                get_str_any(step, &["Association"]),
                get_str_any(step, &["DestinationEntity"]),
            ]
        })
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

fn simple_unconfigured_connector_query(
    documents: &[(String, Document)],
    flow: &Document,
    entity: &str,
) -> bool {
    let actions = nested(flow, "DatabaseConnector$ExecuteDatabaseQueryAction");
    let [action] = actions.as_slice() else {
        return false;
    };
    let qualified = action.get_str("Query").unwrap_or_default();
    let Some((connection_name, query_name)) = qualified.rsplit_once('.') else {
        return false;
    };
    let Some(connection) = documents.iter().find_map(|(module, document)| {
        (document.get_str("$Type").ok() == Some("DatabaseConnector$DatabaseConnection")
            && format!("{module}.{}", document.get_str("Name").unwrap_or_default())
                == connection_name)
            .then_some(document)
    }) else {
        return false;
    };
    let Some(query) = array_docs(connection, &["Queries"])
        .into_iter()
        .find(|query| query.get_str("Name").ok() == Some(query_name))
    else {
        return false;
    };
    if !simple_select(query.get_str("Query").unwrap_or_default()) {
        return false;
    }
    let mappings = array_docs(&query, &["TableMappings"]);
    if mappings.len() != 1 || mappings[0].get_str("Entity").ok() != Some(entity) {
        return false;
    }
    ["ConnectionString", "UserName", "Password"]
        .iter()
        .all(|field| {
            let constant_name = connection.get_str(field).unwrap_or_default();
            !constant_name.is_empty()
                && qualified_unit(documents, "Constants$Constant", constant_name).is_some_and(
                    |constant| {
                        constant
                            .get_str("DefaultValue")
                            .unwrap_or_default()
                            .is_empty()
                    },
                )
        })
}

fn simple_select(query: &str) -> bool {
    let normalized = query
        .trim()
        .trim_end_matches(';')
        .trim()
        .to_ascii_lowercase();
    let Some(rest) = normalized.strip_prefix("select ") else {
        return false;
    };
    let Some((columns, table)) = rest.rsplit_once(" from ") else {
        return false;
    };
    !columns.trim().is_empty() && !columns.contains(';') && is_identifier(table.trim())
}

fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn nested_first(document: &Document, type_name: &str) -> Option<Document> {
    nested(document, type_name).into_iter().next()
}

fn nested(document: &Document, type_name: &str) -> Vec<Document> {
    fn visit(value: &Bson, type_name: &str, result: &mut Vec<Document>) {
        match value {
            Bson::Document(document) => {
                if document.get_str("$Type").ok() == Some(type_name) {
                    result.push(document.clone());
                }
                for value in document.values() {
                    visit(value, type_name, result);
                }
            }
            Bson::Array(values) => {
                for value in values {
                    visit(value, type_name, result);
                }
            }
            _ => {}
        }
    }

    let mut result = Vec::new();
    visit(&Bson::Document(document.clone()), type_name, &mut result);
    result
}

#[cfg(test)]
mod tests {
    use mxrs_bson::doc;

    use super::*;

    fn documents(values: Vec<(&str, Document)>) -> Vec<(String, Document)> {
        values
            .into_iter()
            .map(|(module, document)| (module.to_string(), document))
            .collect()
    }

    #[test]
    fn resolves_xpath_and_association_sources() {
        let xpath = doc! { "DataSource": {
            "$Type": "CustomWidgets$CustomWidgetXPathSource",
            "EntityRef": { "Entity": "Sales.Order" },
            "XPathConstraint": "[Active = true()]",
        }};
        let source = WebListDataSource::from_documents(&[], &xpath);
        assert!(source.supported());
        assert!(source.xpath());
        assert_eq!(source.entity, "Sales.Order");
        assert_eq!(source.xpath_constraint, "[Active = true()]");

        let association = doc! { "DataSource": {
            "$Type": "Forms$AssociationSource",
            "EntityRef": { "Steps": mxrs_bson::build_array(vec![Bson::Document(doc! {
                "Association": "Sales.Order_Customer",
                "DestinationEntity": "Sales.Customer",
            })], 2) },
        }};
        let source = WebListDataSource::from_documents(&[], &association);
        assert!(source.association());
        assert_eq!(source.entity, "Sales.Customer");
        assert_eq!(
            source.association_path,
            "Sales.Order_Customer/Sales.Customer"
        );
    }

    #[test]
    fn requires_referenced_flow_and_uses_its_return_entity() {
        let docs = documents(vec![(
            "Sales",
            doc! {
                "$Type": "Microflows$Microflow",
                "Name": "LoadOrders",
                "MicroflowReturnType": { "Entity": "Sales.Order" },
            },
        )]);
        let widget = doc! { "DataSource": {
            "$Type": "Forms$MicroflowSource",
            "MicroflowSettings": { "Microflow": "Sales.LoadOrders" },
        }};
        let source = WebListDataSource::from_documents(&docs, &widget);
        assert!(source.microflow());
        assert_eq!(source.microflow_name, "Sales.LoadOrders");
        assert_eq!(source.entity, "Sales.Order");
    }

    #[test]
    fn lowers_only_a_simple_unconfigured_connector_select() {
        let empty_constant = |name: &str| {
            doc! {
                "$Type": "Constants$Constant",
                "Name": name,
                "DefaultValue": "",
            }
        };
        let docs = documents(vec![
            (
                "Sales",
                doc! {
                    "$Type": "Microflows$Microflow",
                    "Name": "LoadOrders",
                    "MicroflowReturnType": { "Entity": "Sales.Order" },
                    "Action": {
                        "$Type": "DatabaseConnector$ExecuteDatabaseQueryAction",
                        "Query": "Sales.Connection.GetOrders",
                    },
                },
            ),
            (
                "Sales",
                doc! {
                    "$Type": "DatabaseConnector$DatabaseConnection",
                    "Name": "Connection",
                    "ConnectionString": "Sales.ConnectionString",
                    "UserName": "Sales.UserName",
                    "Password": "Sales.Password",
                    "Queries": mxrs_bson::build_array(vec![Bson::Document(doc! {
                        "Name": "GetOrders",
                        "Query": "SELECT id FROM orders;",
                        "TableMappings": mxrs_bson::build_array(vec![Bson::Document(doc! {
                            "Entity": "Sales.Order",
                        })], 2),
                    })], 2),
                },
            ),
            ("Sales", empty_constant("ConnectionString")),
            ("Sales", empty_constant("UserName")),
            ("Sales", empty_constant("Password")),
        ]);
        let widget = doc! { "DataSource": {
            "$Type": "Forms$MicroflowSource",
            "Microflow": "Sales.LoadOrders",
        }};
        let source = WebListDataSource::from_documents(&docs, &widget);
        assert!(source.xpath());
        assert!(!source.microflow());
        assert_eq!(source.entity, "Sales.Order");
    }
}
