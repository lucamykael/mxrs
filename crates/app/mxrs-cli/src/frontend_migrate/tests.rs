use std::io::Write;
use std::path::{Path, PathBuf};

use mxrs_bson::{Bson, Document, doc};
use mxrs_mpr::MprFile;
use mxrs_widget_package::WidgetPackage;

use super::layout::normalize_weights;
use super::template::{default_value, template};
use super::tree::{self, array};
use super::widget::{
    AUDITED_DATA_GRID_PACKAGE_SHA256, apply_audited_package_migration,
    clear_inactive_default_captions, convert_value, rebind_object, rebind_value, safe_envelope,
};
use super::*;

const WIDGET_ID: &str = "example.Widget";

fn widget_xml(properties: &[(&str, &str, Option<&str>)]) -> String {
    let body: String = properties
        .iter()
        .map(|(key, kind, default)| {
            let default = default.map_or(String::new(), |value| format!(r#" defaultValue="{value}""#));
            format!(r#"<property key="{key}" type="{kind}"{default}><caption>{key}</caption></property>"#)
        })
        .collect();
    format!(
        r#"<widget id="{WIDGET_ID}" supportedPlatform="Web" pluginWidget="true">
  <name>Example</name><properties><propertyGroup caption="General">{body}</propertyGroup></properties>
</widget>"#
    )
}

fn package(path: &Path, xml: &str) {
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    zip.start_file("Widget.xml", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(xml.as_bytes()).unwrap();
    zip.finish().unwrap();
}

fn install(dir: &Path, name: &str, xml: &str) {
    std::fs::create_dir_all(dir.join("widgets")).unwrap();
    package(&dir.join("widgets").join(name), xml);
}

fn project(dir: &Path, version: &str) -> PathBuf {
    let path = dir.join("App.mpr");
    let hash = mxrs_schema::schema_hash(version).unwrap_or_default();
    MprFile::create(&path, version, hash).unwrap();
    path
}

fn insert(path: &Path, document: Document) -> String {
    let mut mpr = MprFile::open(path, false).unwrap();
    let root = mpr.root_unit().unwrap().unwrap().unit_id;
    mpr.insert_unit(&root, "Documents", document, None).unwrap()
}

fn stored(path: &Path, unit_id: &str) -> Document {
    let mpr = MprFile::open(path, true).unwrap();
    let unit = mpr.unit(unit_id).unwrap().unwrap();
    mpr.parse_contents(&unit).unwrap()
}

/// A widget as an older version of the package stored it.
fn old_widget(dir: &Path, properties: &[(&str, &str, Option<&str>)]) -> Document {
    let old = dir.join("old.mpk");
    package(&old, &widget_xml(properties));
    let definition = WidgetPackage::new(&old)
        .definition(WIDGET_ID)
        .unwrap()
        .unwrap();
    let (widget_type, object) = template(&definition);
    doc! {
        "$ID": uuid::Uuid::new_v4().to_string(),
        "$Type": "CustomWidgets$CustomWidget",
        "Name": "example",
        "Type": widget_type,
        "Object": object,
    }
}

fn page(fields: Document) -> Document {
    let mut page = doc! {
        "$ID": uuid::Uuid::new_v4().to_string(),
        "$Type": "Forms$Page",
        "Name": "Home",
    };
    page.extend(fields);
    page
}

fn first_property_value(widget: &mut Document) -> &mut Document {
    let object = widget.get_document_mut("Object").unwrap();
    let property = &mut tree::items_mut(object.get_mut("Properties"))[0];
    match property {
        Bson::Document(property) => property.get_document_mut("Value").unwrap(),
        _ => panic!("a widget property is a document"),
    }
}

/// Each stored property's key and primitive value, in storage order.
fn values_by_key(widget: &Document) -> Vec<(String, Bson)> {
    let types = tree::items(tree::dig(
        widget.get("Type"),
        &["ObjectType", "PropertyTypes"],
    ));
    tree::items(tree::dig(widget.get("Object"), &["Properties"]))
        .iter()
        .map(|property| {
            let pointer = tree::id(tree::field(property, "TypePointer"));
            let property_type = types
                .iter()
                .find(|property_type| tree::id(tree::field(property_type, "$ID")) == pointer)
                .unwrap();
            (
                tree::to_text(tree::field(property_type, "PropertyKey")),
                tree::dig(Some(property), &["Value", "PrimitiveValue"])
                    .cloned()
                    .unwrap(),
            )
        })
        .collect()
}

fn kinds(plan: &MigrationPlan) -> Vec<IssueKind> {
    plan.issues().iter().map(|issue| issue.kind).collect()
}

fn scope(issues: &mut Vec<MigrationIssue>) -> UnitScope<'_> {
    UnitScope {
        unit_id: "u",
        issues,
        counts: Counts::default(),
    }
}

#[test]
fn rebinds_a_widget_to_its_installed_schema_keeping_values_by_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = project(dir.path(), "11.12.1");
    install(
        dir.path(),
        "example.mpk",
        &widget_xml(&[
            ("mode", "enumeration", Some("cards")),
            ("debounce", "integer", Some("300")),
        ]),
    );
    let mut widget = old_widget(dir.path(), &[("mode", "enumeration", Some("list"))]);
    first_property_value(&mut widget).insert("TextTemplate", doc! { "stale": true });
    let unit_id = insert(
        &path,
        page(doc! { "Widgets": array(vec![Bson::Document(widget)], 3) }),
    );

    let mut plan = MigrationPlan::build(&path).unwrap();
    assert!(plan.is_safe(), "{:?}", plan.issues());
    assert_eq!(
        (plan.widgets(), plan.layout_rows(), plan.design_properties()),
        (1, 0, 0)
    );
    assert_eq!(plan.changes()[0].unit_id, unit_id);
    plan.apply().unwrap();
    assert!(plan.is_applied());
    assert!(matches!(
        plan.apply(),
        Err(FrontendMigrationError::AlreadyApplied)
    ));

    let page = stored(&path, &unit_id);
    let Bson::Document(migrated) = &tree::items(page.get("Widgets"))[0] else {
        panic!("the widget is stored");
    };
    assert_eq!(
        values_by_key(migrated),
        [
            ("mode".to_string(), Bson::String("list".into())),
            ("debounce".to_string(), Bson::String("300".into())),
        ]
    );
    let first = &tree::items(tree::dig(migrated.get("Object"), &["Properties"]))[0];
    assert_eq!(
        tree::dig(Some(first), &["Value", "TextTemplate"]),
        Some(&Bson::Null)
    );

    let settled = MigrationPlan::build(&path).unwrap();
    assert!(settled.changes().is_empty() && settled.is_safe());
}

#[test]
fn a_widget_already_on_its_installed_schema_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = project(dir.path(), "11.12.1");
    let properties = [("mode", "enumeration", Some("list"))];
    install(dir.path(), "example.mpk", &widget_xml(&properties));
    insert(
        &path,
        page(doc! { "Widget": old_widget(dir.path(), &properties) }),
    );

    let plan = MigrationPlan::build(&path).unwrap();
    assert!(plan.is_safe());
    assert!(plan.changes().is_empty());
}

#[test]
fn a_removed_configured_property_blocks_the_plan() {
    let dir = tempfile::tempdir().unwrap();
    let path = project(dir.path(), "11.12.1");
    install(
        dir.path(),
        "example.mpk",
        &widget_xml(&[("replacement", "string", None)]),
    );
    let mut widget = old_widget(dir.path(), &[("legacy", "string", None)]);
    first_property_value(&mut widget).insert("PrimitiveValue", "important");
    insert(&path, page(doc! { "Widget": widget }));

    let mut plan = MigrationPlan::build(&path).unwrap();
    assert!(!plan.is_safe());
    let issue = &plan.issues()[0];
    assert_eq!(issue.kind, IssueKind::RemovedConfiguredWidgetProperty);
    assert_eq!(issue.path, "$.Widget");
    assert_eq!(
        issue.message,
        r#"installed schema removed configured property "legacy""#
    );
    let error = plan.apply().unwrap_err().to_string();
    assert!(
        error.starts_with("frontend migration is blocked: removed_configured_widget_property at "),
        "{error}"
    );
}

#[test]
fn an_unconfigured_removed_property_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let path = project(dir.path(), "11.12.1");
    install(
        dir.path(),
        "example.mpk",
        &widget_xml(&[("replacement", "string", None)]),
    );
    insert(
        &path,
        page(doc! { "Widget": old_widget(dir.path(), &[("legacy", "string", None)]) }),
    );

    let plan = MigrationPlan::build(&path).unwrap();
    assert!(plan.is_safe(), "{:?}", plan.issues());
    assert_eq!(plan.widgets(), 1);
}

#[test]
fn a_boolean_becomes_the_expression_it_states() {
    let dir = tempfile::tempdir().unwrap();
    let path = project(dir.path(), "11.12.1");
    install(
        dir.path(),
        "example.mpk",
        &widget_xml(&[("required", "expression", Some("false"))]),
    );
    let mut widget = old_widget(dir.path(), &[("required", "boolean", Some("false"))]);
    first_property_value(&mut widget).insert("PrimitiveValue", "true");
    let unit_id = insert(&path, page(doc! { "Widget": widget }));

    let mut plan = MigrationPlan::build(&path).unwrap();
    assert!(plan.is_safe(), "{:?}", plan.issues());
    plan.apply().unwrap();
    let page = stored(&path, &unit_id);
    let property = &tree::items(tree::dig(page.get("Widget"), &["Object", "Properties"]))[0];
    let value = tree::field(property, "Value").unwrap();
    assert!(tree::is_str(tree::field(value, "Expression"), "true"));
    assert!(tree::is_str(tree::field(value, "PrimitiveValue"), ""));
}

#[test]
fn a_type_change_without_a_lossless_conversion_blocks_the_plan() {
    let dir = tempfile::tempdir().unwrap();
    let path = project(dir.path(), "11.12.1");
    install(
        dir.path(),
        "example.mpk",
        &widget_xml(&[("count", "integer", None)]),
    );
    insert(
        &path,
        page(doc! { "Widget": old_widget(dir.path(), &[("count", "string", None)]) }),
    );

    let plan = MigrationPlan::build(&path).unwrap();
    let issue = &plan.issues()[0];
    assert_eq!(issue.kind, IssueKind::ChangedWidgetProperty);
    assert_eq!(issue.path, "$.Widget.count");
    assert_eq!(
        issue.message,
        "property changed from String to Integer without a lossless conversion"
    );
}

#[test]
fn a_carried_over_data_source_gains_the_fields_the_model_requires() {
    let dir = tempfile::tempdir().unwrap();
    let path = project(dir.path(), "11.12.1");
    let properties = [("source", "datasource", None)];
    install(dir.path(), "example.mpk", &widget_xml(&properties));
    let mut widget = old_widget(dir.path(), &properties);
    first_property_value(&mut widget).insert(
        "DataSource",
        doc! { "$Type": "Forms$MicroflowSettings", "Microflow": "Demo.Source" },
    );
    let unit_id = insert(&path, page(doc! { "Widget": widget }));

    let mut plan = MigrationPlan::build(&path).unwrap();
    assert!(plan.is_safe());
    assert_eq!(plan.widgets(), 1);
    plan.apply().unwrap();
    let page = stored(&path, &unit_id);
    let property = &tree::items(tree::dig(page.get("Widget"), &["Object", "Properties"]))[0];
    let mappings = tree::dig(Some(property), &["Value", "DataSource", "OutputMappings"]);
    assert_eq!(tree::marker(mappings), 3);
    assert!(tree::items(mappings).is_empty());
    assert!(MigrationPlan::build(&path).unwrap().changes().is_empty());
}

#[test]
fn missing_ambiguous_and_unreadable_packages_block_the_plan() {
    let dir = tempfile::tempdir().unwrap();
    let path = project(dir.path(), "11.12.1");
    let properties = [("mode", "enumeration", Some("list"))];
    insert(
        &path,
        page(doc! { "Widget": old_widget(dir.path(), &properties) }),
    );
    let plan = MigrationPlan::build(&path).unwrap();
    assert_eq!(kinds(&plan), [IssueKind::MissingWidgetDefinition]);
    assert_eq!(
        plan.issues()[0].message,
        r#"no installed MPK defines "example.Widget""#
    );

    install(dir.path(), "a.mpk", &widget_xml(&properties));
    install(dir.path(), "b.mpk", &widget_xml(&properties));
    let plan = MigrationPlan::build(&path).unwrap();
    assert_eq!(kinds(&plan), [IssueKind::AmbiguousWidgetDefinition]);
    assert_eq!(
        plan.issues()[0].message,
        r#"multiple installed MPKs define "example.Widget": widgets/a.mpk, widgets/b.mpk"#
    );

    std::fs::remove_file(dir.path().join("widgets/b.mpk")).unwrap();
    std::fs::write(dir.path().join("widgets/broken.mpk"), "not a zip").unwrap();
    let plan = MigrationPlan::build(&path).unwrap();
    assert_eq!(kinds(&plan), [IssueKind::InvalidWidgetPackage]);
    let issue = &plan.issues()[0];
    assert_eq!(
        (issue.unit_id.as_str(), issue.path.as_str()),
        ("", ".widgets.broken.mpk")
    );
    assert!(
        issue
            .message
            .starts_with("cannot read widgets/broken.mpk: ")
    );
}

#[test]
fn a_package_property_of_no_known_type_blocks_its_widget() {
    let dir = tempfile::tempdir().unwrap();
    let path = project(dir.path(), "11.12.1");
    let properties = [("mode", "enumeration", Some("list"))];
    insert(
        &path,
        page(doc! { "Widget": old_widget(dir.path(), &properties) }),
    );
    install(
        dir.path(),
        "example.mpk",
        &widget_xml(&[("mode", "hologram", None)]),
    );

    let plan = MigrationPlan::build(&path).unwrap();
    assert_eq!(kinds(&plan), [IssueKind::InvalidWidgetPackage]);
    assert_eq!(
        plan.issues()[0].message,
        r#"cannot read widgets/example.mpk: widget property "mode" has unrecognized type "hologram""#
    );
}

fn grid_row(weights: &[i32]) -> Bson {
    let columns = weights
        .iter()
        .map(|weight| {
            Bson::Document(doc! {
                "$ID": uuid::Uuid::new_v4().to_string(),
                "$Type": "Forms$LayoutGridColumn",
                "Weight": *weight,
                "TabletWeight": -1,
                "PhoneWeight": -1,
            })
        })
        .collect();
    Bson::Document(doc! {
        "$ID": uuid::Uuid::new_v4().to_string(),
        "$Type": "Forms$LayoutGridRow",
        "Columns": array(columns, 2),
    })
}

#[test]
fn proportional_layout_weights_are_normalized_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = project(dir.path(), "10.24.0.73019");
    let rows: Vec<Bson> = [&[-1][..], &[-2, -1, -2], &[3, -1, -2], &[4, 8]]
        .into_iter()
        .map(grid_row)
        .collect();
    let unit_id = insert(&path, page(doc! { "Rows": rows }));

    let mut plan = MigrationPlan::build(&path).unwrap();
    assert!(plan.is_safe());
    assert_eq!(plan.layout_rows(), 3);
    plan.apply().unwrap();
    let page = stored(&path, &unit_id);
    let Some(Bson::Array(rows)) = page.get("Rows") else {
        panic!("the rows are stored");
    };
    let weights: Vec<Vec<i64>> = rows
        .iter()
        .map(|row| {
            tree::items(tree::field(row, "Columns"))
                .iter()
                .map(|column| tree::integer(tree::field(column, "Weight")).unwrap())
                .collect()
        })
        .collect();
    assert_eq!(
        weights,
        [vec![12], vec![5, 2, 5], vec![3, 3, 6], vec![4, 8]]
    );
    assert!(rows.iter().all(|row| {
        tree::items(tree::field(row, "Columns"))
            .iter()
            .all(|column| tree::integer(tree::field(column, "TabletWeight")) == Some(-1))
    }));
}

#[test]
fn unsafe_layout_weights_and_unsupported_versions_block_the_plan() {
    let dir = tempfile::tempdir().unwrap();
    let path = project(dir.path(), "11.12.1");
    insert(
        &path,
        page(doc! { "Rows": [grid_row(&[0, -1]), grid_row(&[13, -1])] }),
    );
    let plan = MigrationPlan::build(&path).unwrap();
    assert_eq!(
        kinds(&plan),
        [
            IssueKind::UnsafeLayoutWeights,
            IssueKind::UnsafeLayoutWeights
        ]
    );
    assert_eq!(plan.issues()[0].path, "$.Rows[0]");
    assert_eq!(
        plan.issues()[0].message,
        "Weight values cannot be normalized safely: [0, -1]"
    );

    let old = tempfile::tempdir().unwrap();
    let plan = MigrationPlan::build(project(old.path(), "9.24.0")).unwrap();
    assert_eq!(kinds(&plan), [IssueKind::UnsupportedVersion]);
    assert_eq!(
        plan.issues()[0].message,
        r#"Mendix "9.24.0"; supported majors are 10 and 11"#
    );
    assert_eq!(plan.version(), "9.24.0");
}

#[test]
fn weights_normalize_only_losslessly() {
    assert_eq!(normalize_weights(&[1, 11]), None);
    assert_eq!(normalize_weights(&[12, -1]), None);
    assert_eq!(normalize_weights(&[13]), None);
    assert_eq!(normalize_weights(&[10, -1, -1, -1]), None);
    assert_eq!(normalize_weights(&[0, -1]), None);
    assert_eq!(normalize_weights(&[-1, -1]), Some(vec![6, 6]));
    assert_eq!(
        normalize_weights(&[-1, -1, -1, -1, -1]),
        Some(vec![3, 3, 2, 2, 2])
    );
}

fn theme(dir: &Path, catalog: serde_json::Value) {
    let web = dir.join("themesource/atlas_core/web");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("design-properties.json"), catalog.to_string()).unwrap();
}

fn option_value(key: &str, option: &str) -> Bson {
    Bson::Document(design::option_property(key, option))
}

#[test]
fn legacy_spacing_options_fold_into_their_compound_property() {
    let dir = tempfile::tempdir().unwrap();
    let path = project(dir.path(), "11.12.1");
    theme(
        dir.path(),
        serde_json::json!({ "Widget": [{
            "name": "Spacing", "type": "Spacing",
            "margin": [{ "name": "M", "bottom": { "oldNames": ["Spacing bottom::Outer medium"] } }]
        }] }),
    );
    let unit_id = insert(
        &path,
        page(
            doc! { "Appearance": { "DesignProperties": array(vec![option_value("Spacing bottom", "Outer medium")], 2) } },
        ),
    );

    let mut plan = MigrationPlan::build(&path).unwrap();
    assert!(plan.is_safe());
    assert_eq!(plan.design_properties(), 1);
    plan.apply().unwrap();
    let page = stored(&path, &unit_id);
    let properties = tree::items(tree::dig(page.get("Appearance"), &["DesignProperties"]));
    assert_eq!(properties.len(), 1);
    assert!(tree::is_str(tree::field(&properties[0], "Key"), "Spacing"));
    let nested = &tree::items(tree::dig(Some(&properties[0]), &["Value", "Properties"]))[0];
    assert!(tree::is_str(tree::field(nested, "Key"), "margin-bottom"));
    assert!(tree::is_str(
        tree::dig(Some(nested), &["Value", "Option"]),
        "M"
    ));
}

#[test]
fn renamed_design_properties_take_their_new_names() {
    let dir = tempfile::tempdir().unwrap();
    let path = project(dir.path(), "11.12.1");
    theme(
        dir.path(),
        serde_json::json!({ "Widget": [{
            "name": "Card style", "type": "Dropdown", "oldNames": ["Card"],
            "options": [{ "name": "Raised", "oldNames": ["Lifted"] }]
        }] }),
    );
    let unit_id = insert(
        &path,
        page(
            doc! { "Appearance": { "DesignProperties": array(vec![option_value("Card", "Lifted")], 2) } },
        ),
    );

    let mut plan = MigrationPlan::build(&path).unwrap();
    assert_eq!(plan.design_properties(), 1);
    plan.apply().unwrap();
    let page = stored(&path, &unit_id);
    let property = &tree::items(tree::dig(page.get("Appearance"), &["DesignProperties"]))[0];
    assert!(tree::is_str(tree::field(property, "Key"), "Card style"));
    assert!(tree::is_str(
        tree::dig(Some(property), &["Value", "Option"]),
        "Raised"
    ));
}

#[test]
fn a_spacing_option_the_compound_contradicts_blocks_the_plan() {
    let dir = tempfile::tempdir().unwrap();
    let path = project(dir.path(), "11.12.1");
    theme(
        dir.path(),
        serde_json::json!({ "Widget": [{
            "name": "Spacing", "type": "Spacing",
            "margin": [{ "name": "M", "top": { "oldNames": ["Old::M"] } }]
        }] }),
    );
    let mut compound = design::compound_property("Spacing");
    compound.get_document_mut("Value").unwrap().insert(
        "Properties",
        array(vec![option_value("margin-top", "L")], 2),
    );
    insert(
        &path,
        page(
            doc! { "Appearance": { "DesignProperties": array(vec![Bson::Document(compound), option_value("Old", "M")], 2) } },
        ),
    );

    let plan = MigrationPlan::build(&path).unwrap();
    assert_eq!(kinds(&plan), [IssueKind::ConflictingDesignProperty]);
    assert_eq!(plan.issues()[0].path, "$.Appearance");
    assert_eq!(
        plan.issues()[0].message,
        r#""Old::M" conflicts with "margin-top""#
    );
}

fn value_type(kind: &str, default: &str) -> Document {
    doc! {
        "$ID": uuid::Uuid::new_v4().to_string(),
        "$Type": "CustomWidgets$WidgetValueType",
        "Type": kind,
        "DefaultValue": default,
        "Translations": array(Vec::new(), 2),
        "SelectionTypes": array(vec![Bson::String("Single".into()), Bson::String("Multi".into())], 1),
        "ObjectType": Bson::Null,
    }
}

fn property_type(key: &str, value_type: Document) -> Document {
    doc! {
        "$ID": uuid::Uuid::new_v4().to_string(),
        "$Type": "CustomWidgets$WidgetPropertyType",
        "PropertyKey": key,
        "ValueType": value_type,
    }
}

fn object_type(types: &[&Document]) -> Document {
    doc! {
        "$ID": uuid::Uuid::new_v4().to_string(),
        "$Type": "CustomWidgets$WidgetObjectType",
        "PropertyTypes": array(types.iter().map(|value| Bson::Document((*value).clone())).collect(), 2),
    }
}

fn property(property_type: &Document, value: Document) -> Bson {
    Bson::Document(doc! {
        "$ID": uuid::Uuid::new_v4().to_string(),
        "$Type": "CustomWidgets$WidgetProperty",
        "TypePointer": property_type.get("$ID").unwrap().clone(),
        "Value": value,
    })
}

fn object(object_type: &Document, properties: Vec<Bson>) -> Document {
    doc! {
        "$ID": uuid::Uuid::new_v4().to_string(),
        "$Type": "CustomWidgets$WidgetObject",
        "Properties": array(properties, 2),
        "TypePointer": object_type.get("$ID").unwrap().clone(),
    }
}

#[test]
fn a_widget_envelope_must_hold_only_known_fields() {
    let mut issues = Vec::new();
    let mut scope = scope(&mut issues);
    let empty = Document::new();
    assert!(!safe_envelope(
        &Document::new(),
        &empty,
        &empty,
        "$",
        &mut scope
    ));
    let widget = doc! { "Type": { "Mystery": true }, "Object": {} };
    assert!(!safe_envelope(&widget, &empty, &empty, "$", &mut scope));
    let widget = doc! { "Type": {}, "Object": { "Mystery": true, "Another": 1 } };
    assert!(!safe_envelope(&widget, &empty, &empty, "$", &mut scope));
    let widget = doc! { "Type": { "WidgetId": "x" }, "Object": { "Properties": [] } };
    assert!(safe_envelope(&widget, &empty, &empty, "$", &mut scope));

    let messages: Vec<(IssueKind, &str)> = issues
        .iter()
        .map(|issue| (issue.kind, issue.message.as_str()))
        .collect();
    assert_eq!(
        messages,
        [
            (IssueKind::MalformedWidget, "missing Type or Object"),
            (
                IssueKind::UnknownWidgetSchema,
                "unrecognized Type fields: Mystery"
            ),
            (
                IssueKind::UnknownWidgetObject,
                "unrecognized Object fields: Another, Mystery"
            ),
        ]
    );
}

#[test]
fn a_property_pointing_outside_its_schema_blocks_the_rebind() {
    let legacy = property_type("legacy", value_type("String", ""));
    let old_type = object_type(&[&legacy]);
    let new_type = object_type(&[]);
    let stray = doc! { "TypePointer": uuid::Uuid::new_v4().to_string(), "Value": {} };
    let old_object = object(&old_type, vec![Bson::Document(stray)]);
    let mut issues = Vec::new();
    let rebound = rebind_object(
        &Bson::Document(old_object),
        Some(&old_type),
        &object(&new_type, Vec::new()),
        &new_type,
        "$",
        &mut scope(&mut issues),
    );
    assert!(rebound.is_none());
    assert_eq!(issues[0].kind, IssueKind::UnknownPropertyPointer);
    assert!(issues[0].message.starts_with("property pointer \""));
    assert!(issues[0].message.ends_with("\" is not in WidgetType"));
}

#[test]
fn values_and_nested_objects_must_have_their_schema_shape() {
    let string = value_type("String", "");
    let mut issues = Vec::new();
    assert!(rebind_value(None, &string, &string, "$.text", &mut scope(&mut issues)).is_none());

    let nested = value_type("Object", "");
    let mut configured = default_value(&nested);
    configured.insert(
        "Objects",
        array(
            vec![Bson::Document(
                doc! { "$Type": "CustomWidgets$WidgetObject" },
            )],
            2,
        ),
    );
    let unconfigured = default_value(&nested);
    assert!(
        rebind_value(
            Some(&Bson::Document(unconfigured)),
            &nested,
            &nested,
            "$.items",
            &mut scope(&mut issues)
        )
        .is_some()
    );
    assert!(
        rebind_value(
            Some(&Bson::Document(configured)),
            &nested,
            &nested,
            "$.items",
            &mut scope(&mut issues)
        )
        .is_none()
    );

    let boolean = value_type("Boolean", "false");
    let expression = value_type("Expression", "");
    let mut configured = default_value(&boolean);
    configured.insert("PrimitiveValue", "true");
    configured.insert("Image", "configured");
    assert!(
        convert_value(
            Some(&Bson::Document(configured)),
            &boolean,
            &expression,
            "$.flag",
            &mut scope(&mut issues)
        )
        .is_none()
    );

    let found: Vec<(IssueKind, &str, &str)> = issues
        .iter()
        .map(|issue| (issue.kind, issue.path.as_str(), issue.message.as_str()))
        .collect();
    assert_eq!(
        found,
        [
            (
                IssueKind::MalformedWidgetValue,
                "$.text",
                "property Value is not an object"
            ),
            (
                IssueKind::ChangedWidgetObject,
                "$.items",
                "nested Object schema is unavailable"
            ),
            (
                IssueKind::ChangedWidgetProperty,
                "$.flag",
                "property changed from Boolean to Expression without a lossless conversion"
            ),
        ]
    );
}

#[test]
fn nested_objects_rebind_onto_their_new_schema() {
    let old_name = property_type("name", value_type("String", ""));
    let new_name = property_type("name", value_type("String", ""));
    let added = property_type("added", value_type("Integer", "7"));
    let old_child_type = object_type(&[&old_name]);
    let new_child_type = object_type(&[&new_name, &added]);
    let mut old_outer = value_type("Object", "");
    old_outer.insert("ObjectType", old_child_type.clone());
    let mut new_outer = value_type("Object", "");
    new_outer.insert("ObjectType", new_child_type.clone());
    let mut name = default_value(old_name.get_document("ValueType").unwrap());
    name.insert("PrimitiveValue", "kept");
    let child = object(&old_child_type, vec![property(&old_name, name)]);
    let mut outer = default_value(&old_outer);
    outer.insert("Objects", array(vec![Bson::Document(child)], 2));

    let mut issues = Vec::new();
    let rebound = rebind_value(
        Some(&Bson::Document(outer)),
        &old_outer,
        &new_outer,
        "$",
        &mut scope(&mut issues),
    )
    .unwrap();
    assert!(issues.is_empty());
    let objects = tree::items(tree::field(&rebound, "Objects"));
    assert_eq!(objects.len(), 1);
    let properties = tree::items(tree::field(&objects[0], "Properties"));
    let primitives: Vec<&Bson> = properties
        .iter()
        .map(|property| tree::dig(Some(property), &["Value", "PrimitiveValue"]).unwrap())
        .collect();
    assert_eq!(
        primitives,
        [&Bson::String("kept".into()), &Bson::String("7".into())]
    );
}

#[test]
fn a_default_caption_of_a_switched_off_option_is_stored_empty() {
    let mut required_text = value_type("TextTemplate", "");
    required_text.insert("Required", true);
    let caption_type = property_type("selectCaption", required_text);
    let controller_type = property_type("select", value_type("Boolean", "false"));
    let caption_value = default_value(caption_type.get_document("ValueType").unwrap());
    let controller_value = default_value(controller_type.get_document("ValueType").unwrap());
    assert!(tree::truthy(caption_value.get("TextTemplate")));
    let mut custom_value = caption_value.clone();
    custom_value.insert("PrimitiveValue", "custom");
    let as_property =
        |property_type: &Document, value: Document| match property(property_type, value) {
            Bson::Document(property) => property,
            _ => unreachable!("a built property is a document"),
        };
    let caption_bson = Bson::Document(caption_type.clone());
    let controller_bson = Bson::Document(controller_type.clone());
    let types = std::collections::HashMap::from([
        ("selectCaption".to_string(), &caption_bson),
        ("select".to_string(), &controller_bson),
    ]);

    let mut properties = vec![
        (
            "selectCaption".to_string(),
            as_property(&caption_type, caption_value),
        ),
        (
            "select".to_string(),
            as_property(&controller_type, controller_value.clone()),
        ),
    ];
    clear_inactive_default_captions(&mut properties, &types);
    assert_eq!(
        tree::dig(properties[0].1.get("Value"), &["TextTemplate"]),
        Some(&Bson::Null)
    );

    let mut properties = vec![
        (
            "selectCaption".to_string(),
            as_property(&caption_type, custom_value),
        ),
        (
            "select".to_string(),
            as_property(&controller_type, controller_value),
        ),
    ];
    clear_inactive_default_captions(&mut properties, &types);
    assert!(tree::truthy(tree::dig(
        properties[0].1.get("Value"),
        &["TextTemplate"]
    )));
}

#[test]
fn the_audited_data_grid_stores_hidden_texts_empty() {
    let enumeration = value_type("Enumeration", "");
    let selection = value_type("Selection", "");
    let mut text = value_type("TextTemplate", "");
    text.insert("Required", true);
    let column_types: Vec<Document> = ["showContentAs", "dynamicText", "exportValue", "tooltip"]
        .into_iter()
        .map(|key| {
            property_type(
                key,
                if key == "showContentAs" {
                    enumeration.clone()
                } else {
                    text.clone()
                },
            )
        })
        .collect();
    let column_type = object_type(&column_types.iter().collect::<Vec<_>>());
    let mut columns_value_type = value_type("Object", "");
    columns_value_type.insert("ObjectType", column_type.clone());
    let top_types = [
        property_type("columns", columns_value_type),
        property_type("pagination", enumeration.clone()),
        property_type("itemSelection", selection.clone()),
        property_type("loadMoreButtonCaption", text.clone()),
        property_type("clearSelectionButtonLabel", text.clone()),
        property_type("singleSelectionColumnLabel", text.clone()),
    ];
    let top_type = object_type(&top_types.iter().collect::<Vec<_>>());
    let column = |content: &str| {
        let properties = column_types
            .iter()
            .map(|property_type| {
                let mut value = default_value(property_type.get_document("ValueType").unwrap());
                if property_type
                    .get_str("PropertyKey")
                    .is_ok_and(|key| key == "showContentAs")
                {
                    value.insert("PrimitiveValue", content);
                }
                property(property_type, value)
            })
            .collect();
        Bson::Document(object(&column_type, properties))
    };
    let properties = top_types
        .iter()
        .map(|property_type| {
            let mut value = default_value(property_type.get_document("ValueType").unwrap());
            match property_type.get_str("PropertyKey").unwrap() {
                "columns" => {
                    value.insert(
                        "Objects",
                        array(vec![column("dynamicText"), column("customContent")], 2),
                    );
                }
                "itemSelection" => {
                    value.insert("Selection", "Multi");
                }
                _ => {}
            }
            property(property_type, value)
        })
        .collect();
    let widget_type = doc! { "ObjectType": top_type };
    let original = object(widget_type.get_document("ObjectType").unwrap(), properties);

    let mut untouched = original.clone();
    apply_audited_package_migration(&mut untouched, &widget_type, Some("another package"));
    assert!(tree::same_document(&untouched, &original));

    let mut migrated = original;
    apply_audited_package_migration(
        &mut migrated,
        &widget_type,
        Some(AUDITED_DATA_GRID_PACKAGE_SHA256),
    );
    let text_set =
        |property: &Bson| tree::truthy(tree::dig(Some(property), &["Value", "TextTemplate"]));
    let top: Vec<bool> = tree::items(migrated.get("Properties"))
        .iter()
        .map(text_set)
        .collect();
    // Load-more caption and single-selection label are hidden; the
    // multi-selection label is in use.
    assert_eq!(&top[3..], [false, true, false]);
    let columns = tree::items(tree::dig(
        Some(&tree::items(migrated.get("Properties"))[0]),
        &["Value", "Objects"],
    ));
    let texts = |column: &Bson| -> Vec<bool> {
        tree::items(tree::field(column, "Properties"))[1..]
            .iter()
            .map(text_set)
            .collect()
    };
    assert_eq!(texts(&columns[0]), [true, false, true]);
    assert_eq!(texts(&columns[1]), [false, true, false]);
}

#[test]
fn applying_refuses_a_unit_changed_since_the_preview() {
    let dir = tempfile::tempdir().unwrap();
    let path = project(dir.path(), "10.24.0.73019");
    let unit_id = insert(&path, page(doc! { "Row": grid_row(&[-1]) }));
    let mut plan = MigrationPlan::build(&path).unwrap();
    assert_eq!(plan.layout_rows(), 1);

    let mut mpr = MprFile::open(&path, false).unwrap();
    let mut changed = stored(&path, &unit_id);
    changed.insert("Name", "Changed");
    mpr.update_unit(&unit_id, changed).unwrap();
    drop(mpr);

    let error = plan.apply().unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("frontend unit changed after preview: {unit_id}")
    );
    assert!(!plan.is_applied());
    let row = stored(&path, &unit_id);
    let column = &tree::items(tree::dig(row.get("Row"), &["Columns"]))[0];
    assert_eq!(tree::integer(tree::field(column, "Weight")), Some(-1));
}

#[test]
fn the_report_lists_counts_and_issues_in_mxrbs_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = project(dir.path(), "10.24.0.73019");
    insert(&path, page(doc! { "Row": grid_row(&[-1]) }));
    let plan = MigrationPlan::build(&path).unwrap();
    assert_eq!(
        plan.render_text(),
        "Version           : 10.24.0.73019\n\
         Changes           : 1\n\
         Widgets           : 0\n\
         Layout rows       : 1\n\
         Design properties : 0\n\
         Safe              : true\n\
         Applied           : false\n"
    );
    assert_eq!(
        serde_json::to_value(plan.report()).unwrap(),
        serde_json::json!({
            "version": "10.24.0.73019", "changes": 1, "widgets": 0, "layout_rows": 1,
            "design_properties": 0, "issues": [], "safe": true, "applied": false
        })
    );

    let blocked = tempfile::tempdir().unwrap();
    let plan = MigrationPlan::build(project(blocked.path(), "9.24.0")).unwrap();
    assert!(
        plan.render_text().ends_with(
            "[UNSUPPORTED_VERSION] : Mendix \"9.24.0\"; supported majors are 10 and 11\n"
        )
    );
    assert_eq!(
        serde_json::to_value(plan.report()).unwrap()["issues"][0],
        serde_json::json!({
            "unit_id": "", "path": "", "kind": "unsupported_version",
            "message": "Mendix \"9.24.0\"; supported majors are 10 and 11"
        })
    );
}

#[test]
fn ruby_reads_a_version_by_its_leading_integer() {
    assert_eq!(tree::ruby_to_i("11.12.1"), 11);
    assert_eq!(tree::ruby_to_i(" 10.24"), 10);
    assert_eq!(tree::ruby_to_i(""), 0);
    assert_eq!(tree::ruby_to_i("v11"), 0);
    assert_eq!(tree::ruby_to_i("1_1.0"), 11);
}

#[test]
fn a_resolved_definition_carries_its_package_digest() {
    use sha2::{Digest, Sha256};

    let dir = tempfile::tempdir().unwrap();
    install(
        dir.path(),
        "example.mpk",
        &widget_xml(&[("mode", "enumeration", None)]),
    );
    let mut packages = Packages::discover(dir.path());
    let installed = packages.definition(WIDGET_ID).unwrap();
    let bytes = std::fs::read(dir.path().join("widgets/example.mpk")).unwrap();
    let expected: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(installed.digest.as_deref(), Some(expected.as_str()));
    assert_eq!(installed.widget.id, WIDGET_ID);
    assert!(packages.definition("example.Other").is_none());
    assert_eq!(
        packages.failure("example.Other").0,
        IssueKind::MissingWidgetDefinition
    );
}
