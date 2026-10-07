//! Rebinding a pluggable widget to the schema its installed package
//! declares.
//!
//! The package's XML is the source of truth: the widget takes the
//! package's schema, and each value it configures moves to the property of
//! the same key — renumbered onto the new schema, its identities kept, and
//! only the fields its value kind uses carried over. Stored properties keep
//! their order and the package's new ones follow, as Studio Pro stores them
//! (CE0463 checks that order). What cannot move losslessly blocks the plan:
//! a configured property the package dropped, a property whose type changed
//! with no lossless conversion (`Boolean` to `Expression` is the one there
//! is), a value or nested object that does not have the shape its schema
//! says, or stored fields neither Mendix nor the package knows.

use std::collections::HashMap;

use mxrs_bson::{Bson, Document};

use super::packages::Packages;
use super::template::{default_value, object_template, template};
use super::{IssueKind, UnitScope, tree};

/// `CustomWidgets$CustomWidgetType` fields Mendix itself stores, whatever
/// the package declares.
const STANDARD_TYPE_KEYS: [&str; 13] = [
    "$ID",
    "$Type",
    "HelpUrl",
    "ObjectType",
    "OfflineCapable",
    "StudioCategory",
    "StudioProCategory",
    "SupportedPlatform",
    "WidgetDescription",
    "WidgetId",
    "WidgetName",
    "WidgetNeedsEntityContext",
    "WidgetPluginWidget",
];

/// `CustomWidgets$WidgetObject` fields Mendix itself stores.
const STANDARD_OBJECT_KEYS: [&str; 4] = ["$ID", "$Type", "Properties", "TypePointer"];

/// The SHA-256 of the one Data Grid 2 package whose text visibility rules
/// were audited: with it installed, a text the grid's own settings hide is
/// stored empty, as Studio Pro stores it.
pub(super) const AUDITED_DATA_GRID_PACKAGE_SHA256: &str =
    "8eabd99ef927b918dbb2bd5f05cf12dc14026b37e496bc1b0569c34369a0b713";

/// The value fields each value kind uses; any other kind uses
/// `PrimitiveValue`.
fn active_fields(kind: Option<&str>) -> &'static [&'static str] {
    match kind {
        Some("Action") => &["Action", "Microflow", "Nanoflow"],
        Some("Association") => &["EntityRef", "SourceVariable"],
        Some("Attribute") => &["AttributeRef", "SourceVariable"],
        Some("DataSource") => &["DataSource", "XPathConstraint"],
        Some("Expression") => &["Expression", "SourceVariable"],
        Some("Icon") => &["Icon"],
        Some("Image") => &["Image"],
        Some("Object") => &["Objects"],
        Some("Selection") => &["Selection"],
        Some("TextTemplate") => &["TextTemplate", "TranslatableValue"],
        Some("Widgets") => &["Widgets"],
        _ => &["PrimitiveValue"],
    }
}

/// Rebinds a `CustomWidgets$CustomWidget` in place when its installed
/// schema, or what its values hold, differs from what it stores.
pub(super) fn migrate(
    packages: &mut Packages,
    widget: &mut Document,
    path: &str,
    scope: &mut UnitScope<'_>,
) {
    let widget_id = tree::to_text(tree::dig(widget.get("Type"), &["WidgetId"]));
    if widget_id.is_empty() {
        return;
    }
    let Some(installed) = packages.definition(&widget_id) else {
        let (kind, message) = packages.failure(&widget_id);
        scope.issue(path, kind, message);
        return;
    };
    let (new_type, new_object) = template(&installed.widget);
    if !safe_envelope(widget, &new_type, &new_object, path, scope) {
        return;
    }
    let (Some(old_object), Ok(new_object_type)) =
        (widget.get("Object"), new_type.get_document("ObjectType"))
    else {
        return;
    };
    let old_object_type = tree::document(tree::dig(widget.get("Type"), &["ObjectType"]));
    let Some(mut migrated) = rebind_object(
        old_object,
        old_object_type,
        &new_object,
        new_object_type,
        path,
        scope,
    ) else {
        return;
    };
    apply_audited_package_migration(&mut migrated, &new_type, installed.digest.as_deref());
    let new_type = Bson::Document(new_type);
    let migrated = Bson::Document(migrated);
    let schema_changed = !widget
        .get("Type")
        .is_some_and(|stored| tree::same_schema(stored, &new_type));
    let object_changed = !tree::same_schema(old_object, &migrated);
    if schema_changed || object_changed {
        widget.insert("Type", new_type);
        widget.insert("Object", migrated);
        scope.counts.widgets += 1;
    }
}

/// Whether the widget stores a schema and an object, each with no field
/// neither Mendix nor the package declares.
pub(super) fn safe_envelope(
    widget: &Document,
    new_type: &Document,
    new_object: &Document,
    path: &str,
    scope: &mut UnitScope<'_>,
) -> bool {
    let (Some(Bson::Document(old_type)), Some(Bson::Document(old_object))) =
        (widget.get("Type"), widget.get("Object"))
    else {
        scope.issue(path, IssueKind::MalformedWidget, "missing Type or Object");
        return false;
    };
    let extra = |stored: &Document, standard: &[&str], declared: &Document| {
        let mut extra: Vec<&str> = stored
            .keys()
            .map(String::as_str)
            .filter(|key| !standard.contains(key) && !declared.contains_key(*key))
            .collect();
        extra.sort_unstable();
        extra.join(", ")
    };
    let extra_type = extra(old_type, &STANDARD_TYPE_KEYS, new_type);
    if !extra_type.is_empty() {
        scope.issue(
            path,
            IssueKind::UnknownWidgetSchema,
            format!("unrecognized Type fields: {extra_type}"),
        );
        return false;
    }
    let extra_object = extra(old_object, &STANDARD_OBJECT_KEYS, new_object);
    if !extra_object.is_empty() {
        scope.issue(
            path,
            IssueKind::UnknownWidgetObject,
            format!("unrecognized Object fields: {extra_object}"),
        );
        return false;
    }
    true
}

/// A stored property type's value type; an empty one when it has none.
fn value_type_of(property_type: &Bson) -> Document {
    tree::document(tree::field(property_type, "ValueType"))
        .cloned()
        .unwrap_or_default()
}

/// Sets `key` in an ordered list of properties by key, keeping the place of
/// one already there.
fn upsert(properties: &mut Vec<(String, Document)>, key: String, property: Document) {
    match properties.iter_mut().find(|(existing, _)| *existing == key) {
        Some((_, slot)) => *slot = property,
        None => properties.push((key, property)),
    }
}

/// The object `old_object` configures, moved onto `new_type`: each stored
/// value under the property of its key, then the new properties of
/// `new_object` the old one did not have. Nothing when a value cannot
/// move, which has then been reported.
pub(super) fn rebind_object(
    old_object: &Bson,
    old_type: Option<&Document>,
    new_object: &Document,
    new_type: &Document,
    path: &str,
    scope: &mut UnitScope<'_>,
) -> Option<Document> {
    let old_types = old_type.map_or(&[][..], |old_type| {
        tree::items(old_type.get("PropertyTypes"))
    });
    let new_types = tree::items(new_type.get("PropertyTypes"));
    let old_by_pointer: HashMap<Option<String>, &Bson> = old_types
        .iter()
        .map(|property_type| (tree::id(tree::field(property_type, "$ID")), property_type))
        .collect();
    let new_by_key: HashMap<String, &Bson> = new_types
        .iter()
        .map(|property_type| {
            (
                tree::to_text(tree::field(property_type, "PropertyKey")),
                property_type,
            )
        })
        .collect();

    let mut rebound: Vec<(String, Document)> = Vec::new();
    for property in tree::items(tree::field(old_object, "Properties")) {
        let pointer = tree::id(tree::field(property, "TypePointer"));
        let Some(old_property_type) = old_by_pointer.get(&pointer) else {
            scope.issue(
                path,
                IssueKind::UnknownPropertyPointer,
                format!(
                    "property pointer {} is not in WidgetType",
                    pointer
                        .as_deref()
                        .map_or("nil".to_string(), tree::inspect_str)
                ),
            );
            return None;
        };
        let key = tree::to_text(tree::field(old_property_type, "PropertyKey"));
        let old_value_type = value_type_of(old_property_type);
        let value = tree::field(property, "Value");
        let Some(new_property_type) = new_by_key.get(&key) else {
            if is_default_value(value, &old_value_type) {
                continue;
            }
            scope.issue(
                path,
                IssueKind::RemovedConfiguredWidgetProperty,
                format!(
                    "installed schema removed configured property {}",
                    tree::inspect_str(&key)
                ),
            );
            return None;
        };
        let new_value_type = value_type_of(new_property_type);
        let value_path = format!("{path}.{key}");
        let kind = |value_type: &Document| {
            value_type
                .get("Type")
                .filter(|kind| !matches!(kind, Bson::Null))
                .cloned()
        };
        let same_kind = match (kind(&old_value_type), kind(&new_value_type)) {
            (None, None) => true,
            (Some(old), Some(new)) => tree::same(&old, &new),
            _ => false,
        };
        let value = if same_kind {
            rebind_value(value, &old_value_type, &new_value_type, &value_path, scope)
        } else {
            convert_value(value, &old_value_type, &new_value_type, &value_path, scope)
        }?;
        let mut property = tree::document(Some(property)).cloned().unwrap_or_default();
        property.insert(
            "TypePointer",
            tree::field(new_property_type, "$ID")
                .cloned()
                .unwrap_or(Bson::Null),
        );
        property.insert("Value", value);
        upsert(&mut rebound, key, property);
    }

    let mut result = new_object.clone();
    if let Some(id) = tree::field(old_object, "$ID") {
        result.insert("$ID", id.clone());
    }
    let defaults: Vec<(String, &Bson)> = tree::items(new_object.get("Properties"))
        .iter()
        .filter_map(|property| {
            let pointer = tree::id(tree::field(property, "TypePointer"));
            let property_type = new_types
                .iter()
                .find(|property_type| tree::id(tree::field(property_type, "$ID")) == pointer)?;
            Some((
                tree::to_text(tree::field(property_type, "PropertyKey")),
                property,
            ))
        })
        .collect();
    clear_inactive_default_captions(&mut rebound, &new_by_key);
    let added: Vec<Bson> = new_types
        .iter()
        .filter_map(|property_type| {
            let key = tree::to_text(tree::field(property_type, "PropertyKey"));
            if rebound.iter().any(|(existing, _)| *existing == key) {
                return None;
            }
            defaults
                .iter()
                .rev()
                .find(|(default_key, _)| *default_key == key)
                .map(|(_, property)| (*property).clone())
        })
        .collect();
    let values = rebound
        .into_iter()
        .map(|(_, property)| Bson::Document(property))
        .chain(added)
        .collect();
    result.insert(
        "Properties",
        tree::array(values, tree::marker(new_object.get("Properties"))),
    );
    Some(result)
}

/// Whether a stored value is what its value type holds unconfigured.
fn is_default_value(value: Option<&Bson>, value_type: &Document) -> bool {
    tree::same_schema(
        value.unwrap_or(&Bson::Null),
        &Bson::Document(default_value(value_type)),
    )
}

/// A `…Caption` text left at its default while the switch it labels
/// (`…`) is off is stored empty, as Studio Pro stores it.
pub(super) fn clear_inactive_default_captions(
    properties: &mut [(String, Document)],
    types: &HashMap<String, &Bson>,
) {
    for index in 0..properties.len() {
        let (key, property) = &properties[index];
        let Some(controller_key) = key.strip_suffix("Caption") else {
            continue;
        };
        if !tree::truthy(tree::dig(property.get("Value"), &["TextTemplate"])) {
            continue;
        }
        let controller = properties
            .iter()
            .find(|(candidate, _)| candidate == controller_key);
        if !controller.is_some_and(|(_, controller)| {
            tree::is_str(
                tree::dig(controller.get("Value"), &["PrimitiveValue"]),
                "false",
            )
        }) {
            continue;
        }
        let Some(value_type) = types
            .get(key)
            .and_then(|property_type| tree::document(tree::field(property_type, "ValueType")))
        else {
            continue;
        };
        let mut replacement = default_value(value_type);
        if !tree::same_schema(
            property.get("Value").unwrap_or(&Bson::Null),
            &Bson::Document(replacement.clone()),
        ) {
            continue;
        }
        replacement.insert(
            "$ID",
            tree::dig(property.get("Value"), &["$ID"])
                .cloned()
                .unwrap_or(Bson::Null),
        );
        replacement.insert(
            "TypePointer",
            value_type.get("$ID").cloned().unwrap_or(Bson::Null),
        );
        replacement.insert("TextTemplate", Bson::Null);
        properties[index].1.insert("Value", replacement);
    }
}

/// A value of an unchanged kind, moved onto `new_type`; a nested object
/// list is rebound object by object.
pub(super) fn rebind_value(
    old_value: Option<&Bson>,
    old_type: &Document,
    new_type: &Document,
    path: &str,
    scope: &mut UnitScope<'_>,
) -> Option<Bson> {
    let Some(Bson::Document(old_value)) = old_value else {
        scope.issue(
            path,
            IssueKind::MalformedWidgetValue,
            "property Value is not an object",
        );
        return None;
    };
    let mut result = normalized_value(old_value, new_type);
    result.insert(
        "TypePointer",
        new_type.get("$ID").cloned().unwrap_or(Bson::Null),
    );
    if !tree::is_str(old_type.get("Type"), "Object") {
        return Some(Bson::Document(result));
    }
    let objects = tree::items(old_value.get("Objects"));
    let objects_marker = tree::marker(old_value.get("Objects"));
    if objects.is_empty() {
        result.insert("Objects", tree::array(Vec::new(), objects_marker));
        return Some(Bson::Document(result));
    }
    let (Ok(old_object_type), Ok(new_object_type)) = (
        old_type.get_document("ObjectType"),
        new_type.get_document("ObjectType"),
    ) else {
        scope.issue(
            path,
            IssueKind::ChangedWidgetObject,
            "nested Object schema is unavailable",
        );
        return None;
    };
    let mut rebound = Vec::with_capacity(objects.len());
    for (index, object) in objects.iter().enumerate() {
        let object = rebind_object(
            object,
            Some(old_object_type),
            &object_template(new_object_type),
            new_object_type,
            &format!("{path}[{index}]"),
            scope,
        )?;
        rebound.push(Bson::Document(object));
    }
    result.insert("Objects", tree::array(rebound, objects_marker));
    Some(Bson::Document(result))
}

/// The value `value_type` holds unconfigured, carrying over the fields of
/// `old_value` its kind uses and the identities it shares.
fn normalized_value(old_value: &Document, value_type: &Document) -> Document {
    let mut result = default_value(value_type);
    for field in active_fields(value_type.get_str("Type").ok()) {
        if let Some(value) = old_value.get(*field) {
            result.insert(*field, normalize_source_variable(value));
        }
    }
    preserve_ids(&mut result, old_value);
    result
}

/// A value of a changed kind, converted when that is lossless: a `Boolean`
/// that only says `true` or `false` becomes that `Expression`.
pub(super) fn convert_value(
    old_value: Option<&Bson>,
    old_type: &Document,
    new_type: &Document,
    path: &str,
    scope: &mut UnitScope<'_>,
) -> Option<Bson> {
    if tree::is_str(old_type.get("Type"), "Boolean")
        && tree::is_str(new_type.get("Type"), "Expression")
        && let Some(Bson::Document(old_value)) = old_value
        && let Some(Bson::String(primitive)) = old_value.get("PrimitiveValue")
        && (primitive == "true" || primitive == "false")
        && is_boolean_value_only(old_value, old_type)
    {
        let mut result = default_value(new_type);
        result.insert("Expression", primitive.clone());
        result.insert(
            "TypePointer",
            new_type.get("$ID").cloned().unwrap_or(Bson::Null),
        );
        preserve_ids(&mut result, old_value);
        return Some(Bson::Document(result));
    }
    scope.issue(
        path,
        IssueKind::ChangedWidgetProperty,
        format!(
            "property changed from {} to {} without a lossless conversion",
            tree::to_text(old_type.get("Type")),
            tree::to_text(new_type.get("Type"))
        ),
    );
    None
}

/// Whether a boolean value configures nothing but its primitive.
fn is_boolean_value_only(value: &Document, value_type: &Document) -> bool {
    let expected = default_value(value_type);
    let mut actual = value.clone();
    actual.insert(
        "PrimitiveValue",
        expected
            .get("PrimitiveValue")
            .cloned()
            .unwrap_or(Bson::Null),
    );
    tree::same_schema(&Bson::Document(actual), &Bson::Document(expected))
}

/// Gives each node of `target` the `$ID` of the node at the same place in
/// `source`, so a rebuilt value keeps the identities it was stored with.
pub(super) fn preserve_ids(target: &mut Document, source: &Document) {
    if target.contains_key("$ID")
        && let Some(id) = source.get("$ID")
    {
        target.insert("$ID", id.clone());
    }
    for (key, child) in target.iter_mut() {
        if key == "TypePointer" {
            continue;
        }
        if let Some(old) = source.get(key) {
            preserve_value_ids(child, old);
        }
    }
}

fn preserve_value_ids(target: &mut Bson, source: &Bson) {
    match (target, source) {
        (Bson::Document(target), Bson::Document(source)) => preserve_ids(target, source),
        (Bson::Array(target), Bson::Array(source)) => {
            for (child, old) in target.iter_mut().zip(source) {
                preserve_value_ids(child, old);
            }
        }
        _ => {}
    }
}

/// A carried-over value with the fields the current model requires that
/// older ones omitted: a snippet parameter's `SubKey`, and the
/// `OutputMappings` of a nanoflow call and of microflow settings.
pub(super) fn normalize_source_variable(value: &Bson) -> Bson {
    match value {
        Bson::Document(document) => {
            let kind = document.get_str("$Type").unwrap_or_default();
            let mut result = Document::new();
            for (key, child) in document {
                result.insert(key.clone(), normalize_source_variable(child));
                match (kind, key.as_str()) {
                    ("Forms$PageVariable", "SnippetParameter")
                        if !document.contains_key("SubKey") =>
                    {
                        result.insert("SubKey", "");
                    }
                    ("Forms$CallNanoflowClientAction", "Nanoflow")
                    | ("Forms$MicroflowSettings", "Microflow")
                        if !document.contains_key("OutputMappings") =>
                    {
                        result.insert("OutputMappings", tree::array(Vec::new(), 3));
                    }
                    _ => {}
                }
            }
            Bson::Document(result)
        }
        Bson::Array(items) => Bson::Array(items.iter().map(normalize_source_variable).collect()),
        other => other.clone(),
    }
}

/// With the audited Data Grid 2 package installed, the texts the grid's own
/// settings hide are stored empty: the load-more caption without
/// `loadMore` pagination, the selection labels of the selection mode not in
/// use, and each column's texts its `showContentAs` does not show.
pub(super) fn apply_audited_package_migration(
    object: &mut Document,
    widget_type: &Document,
    digest: Option<&str>,
) {
    if digest != Some(AUDITED_DATA_GRID_PACKAGE_SHA256) {
        return;
    }
    let types = keyed_types(tree::document(widget_type.get("ObjectType")));
    let properties = keyed_properties(object.get("Properties"), &types);
    let value_of = |key: &str, field: &str| {
        properties.get(key).and_then(|index| {
            tree::dig(
                tree::items(object.get("Properties")).get(*index),
                &["Value", field],
            )
        })
    };
    let paginated = tree::is_str(value_of("pagination", "PrimitiveValue"), "loadMore");
    let multi = tree::is_str(value_of("itemSelection", "Selection"), "Multi");
    let single = tree::is_str(value_of("itemSelection", "Selection"), "Single");
    let column_types = types
        .iter()
        .find(|(key, _)| key == "columns")
        .and_then(|(_, property_type)| {
            tree::document(tree::dig(Some(property_type), &["ValueType", "ObjectType"]))
        })
        .map(|object_type| keyed_types(Some(object_type)))
        .unwrap_or_default();

    let items = tree::items_mut(object.get_mut("Properties"));
    let mut clear = |key: &str| {
        if let Some(index) = properties.get(key) {
            clear_text_template(items.get_mut(*index));
        }
    };
    if !paginated {
        clear("loadMoreButtonCaption");
    }
    if !multi {
        clear("clearSelectionButtonLabel");
    }
    if !single {
        clear("singleSelectionColumnLabel");
    }
    let has_column_type = types.iter().any(|(key, _)| key == "columns");
    let Some(columns) = properties.get("columns").filter(|_| has_column_type) else {
        return;
    };
    let columns = items
        .get_mut(*columns)
        .and_then(|columns| match columns {
            Bson::Document(columns) => columns.get_mut("Value"),
            _ => None,
        })
        .and_then(|value| match value {
            Bson::Document(value) => value.get_mut("Objects"),
            _ => None,
        });
    for column in tree::items_mut(columns) {
        let Bson::Document(column) = column else {
            continue;
        };
        let values = keyed_properties(column.get("Properties"), &column_types);
        let content = values.get("showContentAs").and_then(|index| {
            tree::dig(
                tree::items(column.get("Properties")).get(*index),
                &["Value", "PrimitiveValue"],
            )
        });
        let dynamic = tree::is_str(content, "dynamicText");
        let custom = tree::is_str(content, "customContent");
        let column_items = tree::items_mut(column.get_mut("Properties"));
        let mut clear = |key: &str| {
            if let Some(index) = values.get(key) {
                clear_text_template(column_items.get_mut(*index));
            }
        };
        if !dynamic {
            clear("dynamicText");
        }
        if !custom {
            clear("exportValue");
        }
        if custom {
            clear("tooltip");
        }
    }
}

/// An object type's property types by key, the last of a repeated key
/// kept.
fn keyed_types(object_type: Option<&Document>) -> Vec<(String, Bson)> {
    let mut types: Vec<(String, Bson)> = Vec::new();
    for property_type in
        tree::items(object_type.and_then(|object_type| object_type.get("PropertyTypes")))
    {
        let key = tree::to_text(tree::field(property_type, "PropertyKey"));
        match types.iter_mut().find(|(existing, _)| *existing == key) {
            Some((_, slot)) => *slot = property_type.clone(),
            None => types.push((key, property_type.clone())),
        }
    }
    types
}

/// Where each property of an object sits among its items, by the key of
/// the property type it points at.
fn keyed_properties(properties: Option<&Bson>, types: &[(String, Bson)]) -> HashMap<String, usize> {
    let mut keyed = HashMap::new();
    for (index, property) in tree::items(properties).iter().enumerate() {
        let pointer = tree::id(tree::field(property, "TypePointer"));
        if let Some((key, _)) = types
            .iter()
            .find(|(_, property_type)| tree::id(tree::field(property_type, "$ID")) == pointer)
        {
            keyed.insert(key.clone(), index);
        }
    }
    keyed
}

fn clear_text_template(property: Option<&mut Bson>) {
    if let Some(Bson::Document(property)) = property
        && let Ok(value) = property.get_document_mut("Value")
    {
        value.insert("TextTemplate", Bson::Null);
    }
}
