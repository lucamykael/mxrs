//! The CRUD of an entity from Atlas's templates, as `mxrs page new
//! Module.Entity --template crud --atlas` asks: the overview from `Grid`,
//! whose data grid lists the entity with a column per attribute and whose
//! New, edit and delete buttons do what mxrs's own CRUD does; the edit page
//! from `Form_Vertical_Edit`, whose form holds an input per attribute.
//! What mxrs's own CRUD compiles is what is bound in — the actions, the
//! form's source and its inputs — so the two CRUDs do the same.

use mxrs_bson::{Binary, BinarySubtype, Bson, Document, build_array, parse_array};
use mxrs_ir::page::LayoutRef;

use crate::forms::{CrudAttribute, PageLayout};
use crate::installed_templates::{self, InstalledTemplate};
use crate::templates::{humanize, plural};
use crate::{Result, ScaffoldError};

/// The templates the CRUD is made of.
pub(crate) const OVERVIEW_TEMPLATE: &str = "Grid";
pub(crate) const EDIT_TEMPLATE: &str = "Form_Vertical_Edit";

const DATAGRID: &str = "com.mendix.widget.web.datagrid.Datagrid";

fn invalid(what: &str, reason: impl ToString) -> ScaffoldError {
    ScaffoldError::InvalidProjectSource {
        path: what.to_string(),
        reason: reason.to_string(),
    }
}

/// A fresh `$ID`, as the model stores one.
fn id() -> Bson {
    let bytes =
        mxrs_bson::uuid_to_blob(&uuid::Uuid::new_v4().to_string()).expect("a v4 uuid is a uuid");
    Bson::Binary(Binary {
        subtype: BinarySubtype::Generic,
        bytes: bytes.to_vec(),
    })
}

fn ty(document: &Document) -> &str {
    document.get_str("$Type").unwrap_or_default()
}

/// Visits every document under `value`, depth first, until `visit` is done.
fn walk(value: &mut Bson, visit: &mut dyn FnMut(&mut Document) -> bool) -> bool {
    match value {
        Bson::Document(document) => {
            if visit(document) {
                return true;
            }
            for (_, inner) in document.iter_mut() {
                if walk(inner, visit) {
                    return true;
                }
            }
            false
        }
        Bson::Array(items) => items.iter_mut().any(|item| walk(item, visit)),
        _ => false,
    }
}

fn walk_document(document: &mut Document, visit: &mut dyn FnMut(&mut Document) -> bool) -> bool {
    let mut value = Bson::Document(std::mem::take(document));
    let found = walk(&mut value, visit);
    if let Bson::Document(back) = value {
        *document = back;
    }
    found
}

/// The first document under `document` that `matches`, cloned.
fn find(document: &Document, matches: &dyn Fn(&Document) -> bool) -> Option<Document> {
    let mut found = None;
    let mut copy = document.clone();
    walk_document(&mut copy, &mut |inner| {
        if matches(inner) {
            found = Some(inner.clone());
            true
        } else {
            false
        }
    });
    found
}

/// Gives every document under `document` a fresh `$ID`: a copy that is its own.
fn fresh(mut document: Document) -> Document {
    walk_document(&mut document, &mut |inner| {
        if inner.contains_key("$ID") {
            inner.insert("$ID", id());
        }
        false
    });
    document
}

fn items(document: &Document, key: &str) -> Vec<Bson> {
    parse_array(document.get_array(key).ok().map(Vec::as_slice)).items
}

/// The widget id a pluggable widget's type states.
fn widget_id(widget: &Document) -> &str {
    widget
        .get_document("Type")
        .ok()
        .and_then(|ty| ty.get_str("WidgetId").ok())
        .unwrap_or_default()
}

/// The `$ID` of the property type `key` names in `object_type`, and that type.
fn property_type(object_type: &Document, key: &str) -> Option<(Bson, Document)> {
    items(object_type, "PropertyTypes")
        .into_iter()
        .filter_map(|item| item.as_document().cloned())
        .find(|property| property.get_str("PropertyKey").ok() == Some(key))
        .and_then(|property| Some((property.get("$ID")?.clone(), property)))
}

/// The value of the property of `object` whose type `pointer` names.
fn property_value<'a>(object: &'a mut Document, pointer: &Bson) -> Option<&'a mut Document> {
    let properties = object.get_array_mut("Properties").ok()?;
    for item in properties.iter_mut() {
        if let Bson::Document(property) = item
            && property.get("TypePointer") == Some(pointer)
        {
            return property.get_document_mut("Value").ok();
        }
    }
    None
}

/// What a column shows, as its `showContentAs` property says.
fn shows(column: &Document, pointer: &Bson) -> String {
    items(column, "Properties")
        .iter()
        .filter_map(|item| item.as_document())
        .find(|property| property.get("TypePointer") == Some(pointer))
        .and_then(|property| property.get_document("Value").ok())
        .and_then(|value| value.get_str("PrimitiveValue").ok())
        .unwrap_or("attribute")
        .to_string()
}

fn xpath_source(entity: &str) -> Document {
    let mut entity_ref = Document::new();
    entity_ref.insert("$ID", id());
    entity_ref.insert("$Type", "DomainModels$DirectEntityRef");
    entity_ref.insert("Entity", entity);
    let mut sort_bar = Document::new();
    sort_bar.insert("$ID", id());
    sort_bar.insert("$Type", "Forms$GridSortBar");
    sort_bar.insert("SortItems", build_array(Vec::new(), 2));
    let mut source = Document::new();
    source.insert("$ID", id());
    source.insert("$Type", "CustomWidgets$CustomWidgetXPathSource");
    source.insert("EntityRef", entity_ref);
    source.insert("ForceFullObjects", false);
    source.insert("SortBar", sort_bar);
    source.insert("SourceVariable", Bson::Null);
    source.insert("XPathConstraint", "");
    source
}

fn attribute_ref(qualified: &str) -> Document {
    let mut reference = Document::new();
    reference.insert("$ID", id());
    reference.insert("$Type", "DomainModels$AttributeRef");
    reference.insert("Attribute", qualified);
    reference.insert("EntityRef", Bson::Null);
    reference
}

/// Says `caption` in every language a text's translations have.
fn say(text: &mut Document, caption: &str) {
    walk_document(text, &mut |inner| {
        if ty(inner) == "Texts$Translation" {
            inner.insert("Text", caption);
        }
        false
    });
}

/// Binds the data grid `widget` to `entity`: it lists the entity, with a
/// column per attribute before the template's own columns that show
/// widgets (its edit and delete buttons).
fn bind_datagrid(widget: &mut Document, entity: &str, attributes: &[CrudAttribute]) -> Result<()> {
    let what = format!("Atlas template {OVERVIEW_TEMPLATE}");
    let object_type = widget
        .get_document("Type")
        .and_then(|ty| ty.get_document("ObjectType"))
        .map_err(|error| invalid(&what, format!("its data grid has no object type: {error}")))?
        .clone();
    let (datasource, _) = property_type(&object_type, "datasource")
        .ok_or_else(|| invalid(&what, "its data grid has no datasource property"))?;
    let (columns, columns_type) = property_type(&object_type, "columns")
        .ok_or_else(|| invalid(&what, "its data grid has no columns property"))?;
    let column_type = columns_type
        .get_document("ValueType")
        .and_then(|value| value.get_document("ObjectType"))
        .map_err(|error| invalid(&what, format!("its columns have no object type: {error}")))?
        .clone();
    let pointer = |key: &str| {
        property_type(&column_type, key)
            .map(|(pointer, _)| pointer)
            .ok_or_else(|| invalid(&what, format!("its columns have no {key} property")))
    };
    let (attribute, header, shows_as) = (
        pointer("attribute")?,
        pointer("header")?,
        pointer("showContentAs")?,
    );
    let object = widget
        .get_document_mut("Object")
        .map_err(|error| invalid(&what, format!("its data grid holds no object: {error}")))?;
    property_value(object, &datasource)
        .ok_or_else(|| invalid(&what, "its data grid states no datasource"))?
        .insert("DataSource", xpath_source(entity));
    let value = property_value(object, &columns)
        .ok_or_else(|| invalid(&what, "its data grid states no columns"))?;
    let existing: Vec<Document> = items(value, "Objects")
        .into_iter()
        .filter_map(|item| item.as_document().cloned())
        .collect();
    let prototype = existing
        .iter()
        .find(|column| shows(column, &shows_as) == "attribute")
        .cloned()
        .ok_or_else(|| {
            invalid(
                &what,
                "its data grid has no column showing an attribute to bind from",
            )
        })?;
    let mut bound: Vec<Bson> = Vec::new();
    for CrudAttribute { name, .. } in attributes {
        let mut column = fresh(prototype.clone());
        property_value(&mut column, &attribute)
            .ok_or_else(|| invalid(&what, "its columns state no attribute"))?
            .insert("AttributeRef", attribute_ref(&format!("{entity}.{name}")));
        let caption = humanize(name);
        if let Some(value) = property_value(&mut column, &header)
            && let Ok(template) = value.get_document_mut("TextTemplate")
        {
            say(template, &caption);
        }
        bound.push(Bson::Document(column));
    }
    for column in existing {
        if shows(&column, &shows_as) != "attribute" {
            bound.push(Bson::Document(column));
        }
    }
    value.insert("Objects", build_array(bound, 2));
    Ok(())
}

/// Makes every page variable of `page` the one of the widget `widget`: the
/// grid a row's button gives the page the row of.
fn name_page_variables(page: &mut Document, widget: &str) {
    if widget.is_empty() {
        return;
    }
    walk_document(page, &mut |inner| {
        if ty(inner) == "Forms$PageVariable" && inner.contains_key("Widget") {
            inner.insert("Widget", widget);
        }
        false
    });
}

/// Replaces, in `page`, the action of the first button whose action is of
/// `action_type` with `action`.
fn rebind_button(page: &mut Document, action_type: &str, action: Document) -> bool {
    walk_document(page, &mut |inner| {
        if ty(inner) == "Forms$ActionButton"
            && inner
                .get_document("Action")
                .ok()
                .is_some_and(|held| ty(held) == action_type)
        {
            inner.insert("Action", fresh(action.clone()));
            true
        } else {
            false
        }
    })
}

/// What the CRUD of an entity is made from.
pub(crate) struct Crud<'a> {
    /// The template of the overview, `Grid`.
    pub(crate) overview: &'a InstalledTemplate,
    /// The template of the edit page, `Form_Vertical_Edit`.
    pub(crate) edit: &'a InstalledTemplate,
    /// The Mendix version the pages are written for.
    pub(crate) version: &'a str,
    pub(crate) shown_in: &'a PageLayout,
    /// `Module.Entity`.
    pub(crate) entity: &'a str,
    pub(crate) attributes: &'a [CrudAttribute],
    pub(crate) roles: &'a [String],
    /// mxrs's own CRUD pages, compiled: what binds the templates.
    pub(crate) own_overview: &'a Document,
    pub(crate) own_edit: &'a Document,
}

/// The pages of the CRUD: the overview and the edit page from the
/// templates, bound the way mxrs's own are — the same actions, the same
/// form — and what the templates held that the pages could not keep.
pub(crate) fn pages(crud: &Crud<'_>) -> Result<(Document, Document, Vec<String>)> {
    let Crud {
        overview,
        edit,
        version,
        shown_in,
        entity,
        attributes,
        roles,
        own_overview,
        own_edit,
    } = *crud;
    let layout = LayoutRef::new(shown_in.layout.clone(), shown_in.parameter.as_str());
    let entity_name = entity.rsplit('.').next().unwrap_or(entity);
    let overview_name = own_overview.get_str("Name").unwrap_or_default().to_string();
    let edit_name = own_edit.get_str("Name").unwrap_or_default().to_string();
    let mut notes = Vec::new();

    // The overview: the grid lists the entity; the buttons do what mxrs's do.
    let (mut list, more) =
        installed_templates::page_document(overview, version, &layout, &overview_name, roles)?;
    notes.extend(more);
    let what = format!("Atlas template {OVERVIEW_TEMPLATE}");
    let mut bound = false;
    let mut failure = None;
    let mut grid_name = String::new();
    walk_document(&mut list, &mut |inner| {
        if ty(inner) == "CustomWidgets$CustomWidget" && widget_id(inner) == DATAGRID {
            if let Err(error) = bind_datagrid(inner, entity, attributes) {
                failure = Some(error);
            }
            grid_name = inner.get_str("Name").unwrap_or_default().to_string();
            bound = true;
            true
        } else {
            false
        }
    });
    if let Some(error) = failure {
        return Err(error);
    }
    if !bound {
        return Err(invalid(
            &what,
            "it holds no Data grid 2 to list the entity with",
        ));
    }
    for action_type in ["Forms$CreateObjectClientAction", "Forms$FormAction"] {
        let Some(action) = find(own_overview, &|inner| ty(inner) == action_type) else {
            return Err(invalid(
                &overview_name,
                format!("mxrs's own overview has no {action_type}"),
            ));
        };
        if !rebind_button(&mut list, action_type, action) {
            notes.push(format!(
                "{what}: no button does a {action_type}; the page has none for it"
            ));
        }
    }
    // A row's button gives the page the row: the variable names the grid
    // the row is of, which is the template's, not mxrs's own list.
    name_page_variables(&mut list, &grid_name);
    // The pages are titled as mxrs's own: the entity's name, its plural.
    if let Some(title) = own_overview.get("Title") {
        list.insert("Title", title.clone());
    }
    let title = humanize(&plural(entity_name));
    walk_document(&mut list, &mut |inner| {
        if ty(inner) == "Forms$DynamicText"
            && inner.get_document("Content").is_ok()
            && find(inner, &|text| {
                ty(text) == "Texts$Translation"
                    && text.get_str("Text").ok() == Some("Page header title")
            })
            .is_some()
        {
            if let Ok(content) = inner.get_document_mut("Content") {
                say(content, &title);
            }
            true
        } else {
            false
        }
    });

    // The edit page: the form is about the page's object, with its inputs.
    let (mut form, more) =
        installed_templates::page_document(edit, version, &layout, &edit_name, roles)?;
    notes.extend(more);
    let what = format!("Atlas template {EDIT_TEMPLATE}");
    if let Some(parameters) = own_edit.get("Parameters") {
        form.insert("Parameters", parameters.clone());
    }
    if let Some(title) = own_edit.get("Title") {
        form.insert("Title", title.clone());
    }
    let own_view = find(own_edit, &|inner| ty(inner) == "Forms$DataView")
        .ok_or_else(|| invalid(&edit_name, "mxrs's own edit page has no data view"))?;
    let inputs: Vec<Bson> = items(&own_view, "Widgets")
        .into_iter()
        .filter(|item| {
            item.as_document()
                .is_some_and(|widget| ty(widget) != "Forms$ActionButton")
        })
        .collect();
    let mut bound = false;
    walk_document(&mut form, &mut |inner| {
        if ty(inner) != "Forms$DataView" {
            return false;
        }
        if let Some(source) = own_view.get("DataSource") {
            inner.insert("DataSource", fresh_value(source));
        }
        // The inputs go where the template's placeholders are: the column
        // holding its text boxes.
        let mut placed = false;
        walk_document(inner, &mut |column| {
            if ty(column) == "Forms$LayoutGridColumn"
                && items(column, "Widgets").iter().any(|item| {
                    item.as_document().is_some_and(|widget| {
                        matches!(
                            ty(widget),
                            "Forms$TextBox" | "Forms$TextArea" | "Forms$DatePicker"
                        )
                    })
                })
            {
                column.insert(
                    "Widgets",
                    build_array(inputs.iter().map(fresh_value).collect(), 2),
                );
                placed = true;
                true
            } else {
                false
            }
        });
        if !placed {
            inner.insert(
                "Widgets",
                build_array(inputs.iter().map(fresh_value).collect(), 2),
            );
        }
        bound = true;
        true
    });
    if !bound {
        return Err(invalid(&what, "it holds no data view to put the form in"));
    }
    Ok((list, form, notes))
}

fn fresh_value(value: &Bson) -> Bson {
    match value {
        Bson::Document(document) => Bson::Document(fresh(document.clone())),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(caption: &str) -> Document {
        let mut translation = Document::new();
        translation.insert("$ID", id());
        translation.insert("$Type", "Texts$Translation");
        translation.insert("LanguageCode", "en_US");
        translation.insert("Text", caption);
        let mut text = Document::new();
        text.insert("$ID", id());
        text.insert("$Type", "Texts$Text");
        text.insert("Items", build_array(vec![Bson::Document(translation)], 3));
        text
    }

    fn button(action_type: &str) -> Document {
        let mut action = Document::new();
        action.insert("$ID", id());
        action.insert("$Type", action_type);
        let mut button = Document::new();
        button.insert("$ID", id());
        button.insert("$Type", "Forms$ActionButton");
        button.insert("Action", action);
        button.insert("CaptionTemplate", text("Go"));
        button
    }

    #[test]
    fn a_fresh_copy_has_ids_of_its_own_and_says_what_it_is_told() {
        let original = button("Forms$NoAction");
        let copy = fresh(original.clone());
        assert_ne!(copy.get("$ID"), original.get("$ID"));
        assert_ne!(
            copy.get_document("Action").unwrap().get("$ID"),
            original.get_document("Action").unwrap().get("$ID")
        );
        assert_eq!(ty(copy.get_document("Action").unwrap()), "Forms$NoAction");
        let mut caption = copy.get_document("CaptionTemplate").unwrap().clone();
        say(&mut caption, "Save");
        let translation = items(&caption, "Items")[0].as_document().unwrap().clone();
        assert_eq!(translation.get_str("Text").unwrap(), "Save");
    }

    #[test]
    fn the_first_button_doing_an_action_takes_the_action_it_is_given() {
        let mut page = Document::new();
        page.insert("$ID", id());
        page.insert("$Type", "Forms$Page");
        page.insert(
            "Widgets",
            build_array(
                vec![
                    Bson::Document(button("Forms$DeleteClientAction")),
                    Bson::Document(button("Forms$FormAction")),
                    Bson::Document(button("Forms$FormAction")),
                ],
                2,
            ),
        );
        let mut replacement = Document::new();
        replacement.insert("$ID", id());
        replacement.insert("$Type", "Forms$FormAction");
        replacement.insert("Marker", "bound");
        assert!(rebind_button(
            &mut page,
            "Forms$FormAction",
            replacement.clone()
        ));
        let buttons: Vec<Document> = items(&page, "Widgets")
            .into_iter()
            .map(|item| item.as_document().unwrap().clone())
            .collect();
        let marked = |button: &Document| {
            button
                .get_document("Action")
                .unwrap()
                .get_str("Marker")
                .is_ok()
        };
        assert!(!marked(&buttons[0]) && marked(&buttons[1]) && !marked(&buttons[2]));
        // Its own copy: not the id it was given.
        assert_ne!(
            buttons[1].get_document("Action").unwrap().get("$ID"),
            replacement.get("$ID")
        );
        assert!(!rebind_button(
            &mut page,
            "Forms$CreateObjectClientAction",
            replacement
        ));
        assert!(find(&page, &|inner| ty(inner) == "Forms$DeleteClientAction").is_some());
        assert!(find(&page, &|inner| ty(inner) == "Forms$Nothing").is_none());
    }

    #[test]
    fn a_pages_variables_name_the_grid_a_row_is_of() {
        let mut variable = Document::new();
        variable.insert("$ID", id());
        variable.insert("$Type", "Forms$PageVariable");
        variable.insert("Widget", "list");
        let mut other = Document::new();
        other.insert("$ID", id());
        other.insert("$Type", "Forms$PageVariable");
        other.insert("UseAllPages", true);
        let mut page = Document::new();
        page.insert("$Type", "Forms$Page");
        page.insert(
            "Widgets",
            build_array(vec![Bson::Document(variable), Bson::Document(other)], 2),
        );
        name_page_variables(&mut page, "dataGrid2_1");
        let variables: Vec<Document> = items(&page, "Widgets")
            .into_iter()
            .map(|item| item.as_document().unwrap().clone())
            .collect();
        assert_eq!(variables[0].get_str("Widget").unwrap(), "dataGrid2_1");
        assert!(variables[1].get("Widget").is_none());
        // Nothing to name: nothing changes.
        name_page_variables(&mut page, "");
        assert_eq!(
            items(&page, "Widgets")[0]
                .as_document()
                .unwrap()
                .get_str("Widget")
                .unwrap(),
            "dataGrid2_1"
        );
    }

    #[test]
    fn a_widgets_property_is_found_by_the_key_its_type_names() {
        let mut declared = Document::new();
        declared.insert("$ID", id());
        declared.insert("$Type", "CustomWidgets$WidgetPropertyType");
        declared.insert("PropertyKey", "datasource");
        let pointer = declared.get("$ID").cloned().unwrap();
        let mut object_type = Document::new();
        object_type.insert("$Type", "CustomWidgets$WidgetObjectType");
        object_type.insert(
            "PropertyTypes",
            build_array(vec![Bson::Document(declared)], 2),
        );
        let mut value = Document::new();
        value.insert("$Type", "CustomWidgets$WidgetValue");
        value.insert("DataSource", Bson::Null);
        let mut property = Document::new();
        property.insert("$Type", "CustomWidgets$WidgetProperty");
        property.insert("TypePointer", pointer.clone());
        property.insert("Value", value);
        let mut object = Document::new();
        object.insert("Properties", build_array(vec![Bson::Document(property)], 2));
        let (found, _) = property_type(&object_type, "datasource").unwrap();
        assert_eq!(found, pointer);
        assert!(property_type(&object_type, "columns").is_none());
        property_value(&mut object, &pointer)
            .unwrap()
            .insert("DataSource", xpath_source("Sales.Order"));
        let source = property_value(&mut object, &pointer)
            .unwrap()
            .get_document("DataSource")
            .unwrap()
            .clone();
        assert_eq!(ty(&source), "CustomWidgets$CustomWidgetXPathSource");
        assert_eq!(
            source
                .get_document("EntityRef")
                .unwrap()
                .get_str("Entity")
                .unwrap(),
            "Sales.Order"
        );
        assert_eq!(
            attribute_ref("Sales.Order.Number")
                .get_str("Attribute")
                .unwrap(),
            "Sales.Order.Number"
        );
    }
}
