//! What a page, or a snippet, names inside itself and of the pages it opens,
//! checked against what they declare: the parameter a button's mapping
//! gives a value to, the parameter, variable or widget a page variable
//! names, and the attribute an input of a data view shows. The model stores
//! each as text, so a page that names what is not there builds and fails
//! only when it runs; these are warnings, as a page may be imported so.

use std::collections::{BTreeMap, BTreeSet};

use mxrs_bson::{Bson, Document};

use crate::Diagnostic;

/// What a page or snippet declares that its widgets, and the buttons of
/// other pages, may name.
#[derive(Debug, Default)]
pub(crate) struct FormShape {
    /// `page` or `snippet`, as the diagnostics name the form.
    pub(crate) kind: &'static str,
    /// Its parameters, each with the entity it takes when it takes one.
    pub(crate) parameters: BTreeMap<String, Option<String>>,
    pub(crate) variables: BTreeSet<String>,
    pub(crate) widgets: BTreeSet<String>,
}

impl FormShape {
    pub(crate) fn of(kind: &'static str, document: &Document) -> Self {
        let mut shape = FormShape {
            kind,
            ..FormShape::default()
        };
        for parameter in documents(field(document, "Parameters")) {
            if let Some(name) = text(parameter, "Name") {
                let entity = field(parameter, "ParameterType")
                    .and_then(as_document)
                    .and_then(|kind| text(kind, "Entity"))
                    .map(str::to_string);
                shape.parameters.insert(name.to_string(), entity);
            }
        }
        for variable in documents(field(document, "Variables")) {
            if let Some(name) = text(variable, "Name") {
                shape.variables.insert(name.to_string());
            }
        }
        collect_names(document, &mut shape.widgets);
        shape
    }
}

/// Checks the page or snippet `qualified` against what it and every page
/// declare; `generalizations` gives each entity the one it specializes.
pub(crate) fn check(
    qualified: &str,
    document: &Document,
    forms: &BTreeMap<String, FormShape>,
    generalizations: &BTreeMap<String, String>,
) -> Vec<Diagnostic> {
    let Some(own) = forms.get(qualified) else {
        return Vec::new();
    };
    let mut check = Check {
        from: format!("{}:{qualified}", own.kind),
        qualified,
        own,
        forms,
        generalizations,
        diagnostics: Vec::new(),
    };
    check.walk(document);
    check.diagnostics
}

struct Check<'a> {
    from: String,
    qualified: &'a str,
    own: &'a FormShape,
    forms: &'a BTreeMap<String, FormShape>,
    generalizations: &'a BTreeMap<String, String>,
    diagnostics: Vec<Diagnostic>,
}

impl Check<'_> {
    fn say(&mut self, code: &str, message: String) {
        self.diagnostics.push(Diagnostic {
            severity: "warning".into(),
            code: code.into(),
            message: format!("{} {message}", self.from),
        });
    }

    fn walk(&mut self, document: &Document) {
        let kind = text(document, "$Type").unwrap_or_default();
        if kind == "Forms$PageVariable" {
            self.page_variable(document);
        }
        // A button's page settings: each mapping names a parameter of the
        // page it opens.
        if let Some(page) = text(document, "Form") {
            for mapping in documents(field(document, "ParameterMappings")) {
                self.mapping(page, mapping);
            }
        }
        if kind == "Forms$DataView"
            && let Some(entity) = self.data_view_entity(document)
        {
            for widgets in ["Widgets", "FooterWidgets"] {
                for widget in documents(field(document, widgets)) {
                    self.inputs(&entity, widget);
                }
            }
        }
        for (_, value) in document {
            self.walk_value(value);
        }
    }

    fn walk_value(&mut self, value: &Bson) {
        match value {
            Bson::Document(document) => self.walk(document),
            Bson::Array(values) => values.iter().for_each(|value| self.walk_value(value)),
            _ => {}
        }
    }

    fn mapping(&mut self, page: &str, mapping: &Document) {
        let Some(parameter) = text(mapping, "Parameter") else {
            return;
        };
        let Some(target) = self.forms.get(page) else {
            // A page that is not there is the reference check's to say.
            return;
        };
        match parameter_of(page, parameter) {
            Some(name) if target.parameters.contains_key(name) => {}
            Some(name) => self.say(
                "unknown_page_parameter",
                format!("opens {page} with {name:?}, which is not a parameter of it"),
            ),
            None => self.say(
                "unknown_page_parameter",
                format!("opens {page} with {parameter:?}, a parameter of another page"),
            ),
        }
    }

    fn page_variable(&mut self, variable: &Document) {
        for key in ["PageParameter", "SnippetParameter"] {
            let Some(parameter) = text(variable, key) else {
                continue;
            };
            let name = parameter_of(self.qualified, parameter);
            if !name.is_some_and(|name| self.own.parameters.contains_key(name)) {
                let kind = self.own.kind;
                self.say(
                    "unknown_page_variable",
                    format!("names the parameter {parameter:?}, which this {kind} does not have"),
                );
            }
        }
        if let Some(local) = text(variable, "LocalVariable")
            && !self.own.variables.contains(local)
        {
            let kind = self.own.kind;
            self.say(
                "unknown_page_variable",
                format!("names the variable {local:?}, which this {kind} does not declare"),
            );
        }
        if let Some(widget) = text(variable, "Widget")
            && !self.own.widgets.contains(widget)
        {
            let kind = self.own.kind;
            self.say(
                "unknown_widget",
                format!("names the widget {widget:?}, which this {kind} does not hold"),
            );
        }
    }

    /// The entity a data view shows: the one its source names, the one an
    /// association path from its variable leads to, or that of the
    /// parameter it is given. A source read no other way is not checked.
    fn data_view_entity(&self, view: &Document) -> Option<String> {
        let source = field(view, "DataSource").and_then(as_document)?;
        if text(source, "$Type") != Some("Forms$DataViewSource") {
            return None;
        }
        if let Some(reference) = field(source, "EntityRef").and_then(as_document) {
            if let Some(entity) = text(reference, "Entity") {
                return Some(entity.to_string());
            }
            // Over associations: the entity the last step reaches.
            return documents(field(reference, "Steps"))
                .last()
                .and_then(|step| text(step, "DestinationEntity"))
                .map(str::to_string);
        }
        let parameter = field(source, "SourceVariable")
            .and_then(as_document)
            .and_then(|variable| text(variable, "PageParameter"))?;
        let name = parameter_of(self.qualified, parameter)?;
        self.own.parameters.get(name).cloned().flatten()
    }

    /// The inputs a data view holds, each against the entity it shows:
    /// what holds a context of its own — another data view, a list, a
    /// reference set selector, a pluggable widget, a snippet — is checked
    /// on its own, and an input that names the variable it is about is that
    /// variable's.
    fn inputs(&mut self, entity: &str, widget: &Document) {
        let kind = text(widget, "$Type").unwrap_or_default();
        if matches!(
            kind,
            "Forms$DataView"
                | "Forms$ListView"
                | "Forms$TemplateGrid"
                | "Forms$DataGrid"
                | "Forms$SnippetCallWidget"
                | "Forms$ReferenceSetSelector"
                | "Forms$InputReferenceSetSelector"
                | "CustomWidgets$CustomWidget"
        ) || field(widget, "SourceVariable")
            .is_some_and(|variable| !matches!(variable, Bson::Null))
        {
            return;
        }
        if let Some(reference) = field(widget, "AttributeRef").and_then(as_document)
            && let Some(attribute) = text(reference, "Attribute")
            // An attribute reached over an association is another entity's.
            && field(reference, "EntityRef").is_none_or(|path| matches!(path, Bson::Null))
            && let Some((owner, _)) = attribute.rsplit_once('.')
            && owner.contains('.')
            && !self.is_or_specializes(entity, owner)
        {
            let name = text(widget, "Name").unwrap_or(kind);
            self.say(
                "attribute_outside_context",
                format!(
                    "shows {attribute:?} in {name:?}, inside a data view of {entity}, which has no such attribute"
                ),
            );
        }
        for (_, value) in widget {
            match value {
                Bson::Document(document) => self.inputs(entity, document),
                Bson::Array(values) => {
                    for value in values {
                        if let Bson::Document(document) = value {
                            self.inputs(entity, document);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn is_or_specializes(&self, entity: &str, owner: &str) -> bool {
        let mut current = entity;
        for _ in 0..32 {
            if current.eq_ignore_ascii_case(owner) {
                return true;
            }
            let Some(parent) = self.generalizations.get(current) else {
                return false;
            };
            current = parent;
        }
        false
    }
}

/// The name of a parameter of `form` as a reference states it: qualified
/// by the form (`Module.Page.Parameter`), or by itself, as the store often
/// keeps it.
fn parameter_of<'a>(form: &str, parameter: &'a str) -> Option<&'a str> {
    match parameter
        .strip_prefix(form)
        .and_then(|rest| rest.strip_prefix('.'))
    {
        Some(name) => Some(name),
        None if !parameter.contains('.') => Some(parameter),
        None => None,
    }
}

/// Every name a widget of the form is given.
fn collect_names(document: &Document, names: &mut BTreeSet<String>) {
    for (key, value) in document {
        match value {
            Bson::String(name) if key.eq_ignore_ascii_case("Name") && !name.is_empty() => {
                names.insert(name.clone());
            }
            Bson::Document(document) => collect_names(document, names),
            Bson::Array(values) => {
                for value in values {
                    if let Bson::Document(document) = value {
                        collect_names(document, names);
                    }
                }
            }
            _ => {}
        }
    }
}

/// A field of a stored document, however the store cases its name.
fn field<'a>(document: &'a Document, name: &str) -> Option<&'a Bson> {
    document
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value)
}

fn text<'a>(document: &'a Document, name: &str) -> Option<&'a str> {
    match field(document, name)? {
        Bson::String(text) if !text.is_empty() => Some(text),
        _ => None,
    }
}

fn as_document(value: &Bson) -> Option<&Document> {
    match value {
        Bson::Document(document) => Some(document),
        _ => None,
    }
}

/// The documents of a stored list (the store's first item is the list's
/// kind marker, not a document).
fn documents(value: Option<&Bson>) -> impl Iterator<Item = &Document> {
    let values: &[Bson] = match value {
        Some(Bson::Array(values)) => values,
        _ => &[],
    };
    values.iter().filter_map(as_document)
}

#[cfg(test)]
mod tests {
    use mxrs_bson::{build_array, doc};

    use super::*;

    fn list(documents: Vec<Document>) -> Bson {
        Bson::Array(build_array(
            documents.into_iter().map(Bson::Document).collect(),
            2,
        ))
    }

    fn input(name: &str, attribute: &str) -> Document {
        doc! {
            "$Type": "Forms$TextBox",
            "Name": name,
            "AttributeRef": { "$Type": "DomainModels$AttributeRef", "Attribute": attribute, "EntityRef": Bson::Null },
        }
    }

    fn variable(key: &str, value: &str) -> Document {
        let mut variable = doc! { "$Type": "Forms$PageVariable" };
        variable.insert(key, value);
        variable
    }

    /// A page that names what it, or the page it opens, does not declare is
    /// warned of, each where it is; what it does declare is not.
    #[test]
    fn a_page_names_only_what_it_and_the_pages_it_opens_declare() {
        let edit = doc! {
            "$Type": "Forms$Page",
            "Name": "Pet_Edit",
            "Parameters": list(vec![doc! {
                "$Type": "Forms$PageParameter",
                "Name": "Pet",
                "ParameterType": { "$Type": "DataTypes$ObjectType", "Entity": "Main.Pet" },
            }]),
            "Variables": list(vec![doc! { "$Type": "Forms$LocalVariable", "Name": "Filter" }]),
            "FormCall": { "Arguments": list(vec![doc! { "Widgets": list(vec![doc! {
                "$Type": "Forms$DataView",
                "Name": "dataView1",
                "DataSource": {
                    "$Type": "Forms$DataViewSource",
                    "EntityRef": Bson::Null,
                    "SourceVariable": variable("PageParameter", "Main.Pet_Edit.Pet"),
                },
                "Widgets": list(vec![
                    input("name", "Main.Pet.Name"),
                    doc! {
                        "$Type": "Forms$DataView",
                        "Name": "bare",
                        "DataSource": {
                            "$Type": "Forms$DataViewSource",
                            "SourceVariable": variable("PageParameter", "Pet"),
                        },
                        "Widgets": list(vec![input("stranger", "Main.Customer.Email")]),
                    },
                    input("legs", "Main.Animal.Legs"),
                    input("owner", "Main.Customer.Name"),
                    doc! {
                        "$Type": "Forms$DataView",
                        "Name": "inner",
                        "Widgets": list(vec![input("other", "Main.Customer.Email")]),
                    },
                ]),
            }, doc! {
                "$Type": "Forms$DataView",
                "Name": "owned",
                "DataSource": {
                    "$Type": "Forms$DataViewSource",
                    "EntityRef": {
                        "$Type": "DomainModels$IndirectEntityRef",
                        "Steps": list(vec![doc! {
                            "$Type": "DomainModels$EntityRefStep",
                            "Association": "Main.Pet_Customer",
                            "DestinationEntity": "Main.Customer",
                        }]),
                    },
                    "SourceVariable": variable("PageParameter", "Pet"),
                },
                "Widgets": list(vec![
                    input("owner_name", "Main.Customer.Name"),
                    {
                        let mut elsewhere = input("pet_name", "Main.Pet.Name");
                        elsewhere.insert("SourceVariable", variable("PageParameter", "Pet"));
                        elsewhere
                    },
                    doc! {
                        "$Type": "Forms$ReferenceSetSelector",
                        "Name": "friends",
                        "Columns": list(vec![input("friend", "Main.Friend.Name")]),
                    },
                ]),
            }, doc! {
                "$Type": "Forms$DataView",
                "Name": "lost",
                "DataSource": {
                    "$Type": "Forms$DataViewSource",
                    "SourceVariable": variable("PageParameter", "Main.Pet_Edit.Animal"),
                },
            }, doc! {
                "$Type": "Forms$ListView",
                "Name": "kept",
                "DataSource": {
                    "$Type": "Forms$ListenTargetSource",
                    "ListenTarget": "list",
                },
                "Variables": list(vec![variable("LocalVariable", "Filter"), variable("LocalVariable", "Nope")]),
            }])}])},
        };
        let overview = doc! {
            "$Type": "Forms$Page",
            "Name": "Pet_Overview",
            "FormCall": { "Arguments": list(vec![doc! { "Widgets": list(vec![doc! {
                "$Type": "CustomWidgets$CustomWidget",
                "Name": "dataGrid2_1",
                "Buttons": list(vec![
                    doc! { "Form": "Main.Pet_Edit", "ParameterMappings": list(vec![doc! {
                        "$Type": "Forms$PageParameterMapping",
                        "Parameter": "Main.Pet_Edit.Pet",
                        "Variable": variable("Widget", "dataGrid2_1"),
                    }])},
                    doc! { "Form": "Main.Pet_Edit", "ParameterMappings": list(vec![doc! {
                        "$Type": "Forms$PageParameterMapping",
                        "Parameter": "Main.Pet_Edit.Animal",
                        "Variable": variable("Widget", "gone"),
                    }])},
                    doc! { "Form": "Main.Pet_Edit", "ParameterMappings": list(vec![doc! {
                        "$Type": "Forms$PageParameterMapping",
                        "Parameter": "Main.Other.Pet",
                    }])},
                    doc! { "Form": "Main.Missing", "ParameterMappings": list(vec![doc! {
                        "$Type": "Forms$PageParameterMapping",
                        "Parameter": "Main.Missing.Pet",
                    }])},
                ]),
            }])}])},
        };
        let forms = BTreeMap::from([
            ("Main.Pet_Edit".to_string(), FormShape::of("page", &edit)),
            (
                "Main.Pet_Overview".to_string(),
                FormShape::of("page", &overview),
            ),
        ]);
        let generalizations = BTreeMap::from([("Main.Pet".to_string(), "Main.Animal".to_string())]);
        let messages = |qualified: &str, document: &Document| {
            check(qualified, document, &forms, &generalizations)
                .into_iter()
                .map(|diagnostic| format!("{} {}", diagnostic.code, diagnostic.message))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            messages("Main.Pet_Edit", &edit),
            vec![
                "attribute_outside_context page:Main.Pet_Edit shows \"Main.Customer.Name\" in \"owner\", inside a data view of Main.Pet, which has no such attribute",
                "attribute_outside_context page:Main.Pet_Edit shows \"Main.Customer.Email\" in \"stranger\", inside a data view of Main.Pet, which has no such attribute",
                "unknown_page_variable page:Main.Pet_Edit names the parameter \"Main.Pet_Edit.Animal\", which this page does not have",
                "unknown_page_variable page:Main.Pet_Edit names the variable \"Nope\", which this page does not declare",
            ]
        );
        assert_eq!(
            messages("Main.Pet_Overview", &overview),
            vec![
                "unknown_page_parameter page:Main.Pet_Overview opens Main.Pet_Edit with \"Animal\", which is not a parameter of it",
                "unknown_widget page:Main.Pet_Overview names the widget \"gone\", which this page does not hold",
                "unknown_page_parameter page:Main.Pet_Overview opens Main.Pet_Edit with \"Main.Other.Pet\", a parameter of another page",
            ]
        );
    }
}
