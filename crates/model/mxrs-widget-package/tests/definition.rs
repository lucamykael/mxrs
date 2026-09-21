//! Typed-schema extraction from fixture MPKs, mirroring the behaviors
//! `Mxrb::WidgetPackage`'s own usage relies on: manifest-driven entry
//! selection, `propertyGroup` category flattening, system properties,
//! value-type details, and `find`'s sorted-and-skip-unreadable lookup.

use std::io::Write;
use std::path::Path;

const WIDGET_XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<widget id="com.example.rating.Rating" pluginWidget="true" needsEntityContext="true" offlineCapable="true"
        supportedPlatform="Web" xmlns="http://www.mendix.com/widget/1.0/">
    <name>Rating</name>
    <description>Shows &amp; edits a rating.</description>
    <helpUrl>https://example.com/rating</helpUrl>
    <studioCategory>Input</studioCategory>
    <studioProCategory>Input widgets</studioProCategory>
    <properties>
        <propertyGroup caption="General">
            <property key="rating" type="attribute" required="true" onChange="onChangeAction">
                <caption>Rating attribute</caption>
                <description>Where the value lives.</description>
                <attributeTypes>
                    <attributeType name="Integer"/>
                    <attributeType name="Decimal"/>
                </attributeTypes>
            </property>
            <property key="editable" type="boolean" defaultValue="true">
                <caption>Editable</caption>
                <description/>
            </property>
            <propertyGroup caption="Appearance">
                <property key="icon" type="enumeration" defaultValue="star">
                    <caption>Icon</caption>
                    <description/>
                    <enumerationValues>
                        <enumerationValue key="star">Star</enumerationValue>
                        <enumerationValue key="heart">Heart</enumerationValue>
                    </enumerationValues>
                </property>
            </propertyGroup>
        </propertyGroup>
        <propertyGroup caption="Events">
            <property key="onChangeAction" type="action" required="false">
                <caption>On change</caption>
                <description/>
                <actionVariables>
                    <actionVariable key="newValue" type="Decimal" caption="New value"/>
                </actionVariables>
                <returnType type="Boolean" assignableTo="editable"/>
            </property>
            <property key="columns" type="object" isList="true">
                <caption>Columns</caption>
                <description/>
                <properties>
                    <propertyGroup caption="Column">
                        <property key="caption" type="textTemplate" required="false">
                            <caption>Caption</caption>
                            <description/>
                            <translations>
                                <translation lang="en_US">Column</translation>
                            </translations>
                        </property>
                        <systemProperty key="Visibility"/>
                    </propertyGroup>
                </properties>
            </property>
        </propertyGroup>
        <systemProperty key="Label"/>
    </properties>
</widget>
"#;

fn write_mpk(path: &Path, entries: &[(&str, &str)]) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    for (name, content) in entries {
        zip.start_file(*name, options).unwrap();
        zip.write_all(content.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}

fn manifest(paths: &[&str]) -> String {
    let files: String = paths
        .iter()
        .map(|path| format!("<widgetFile path=\"{path}\"/>"))
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <package xmlns=\"http://www.mendix.com/package/1.0/\">\
         <clientModule name=\"Rating\" version=\"1.0.0\">\
         <widgetFiles>{files}</widgetFiles></clientModule></package>"
    )
}

#[test]
fn a_manifest_declared_definition_is_parsed_into_the_typed_schema() {
    let directory = tempfile::tempdir().unwrap();
    let mpk = directory.path().join("Rating.mpk");
    write_mpk(
        &mpk,
        &[
            ("package.xml", &manifest(&["com/example/rating/Rating.xml"])),
            ("com/example/rating/Rating.xml", WIDGET_XML),
            ("com/example/rating/Rating.js", "not xml"),
        ],
    );
    let definition = mxrs_widget_package::WidgetPackage::new(&mpk)
        .definition("com.example.rating.Rating")
        .unwrap()
        .unwrap();

    assert_eq!(definition.id, "com.example.rating.Rating");
    assert_eq!(definition.name, "Rating");
    assert_eq!(definition.description, "Shows & edits a rating.");
    assert_eq!(definition.help_url, "https://example.com/rating");
    assert_eq!(definition.studio_category, "Input");
    assert_eq!(definition.studio_pro_category, "Input widgets");
    assert_eq!(definition.platform, "Web");
    assert!(definition.offline);
    assert!(definition.needs_context);
    assert!(definition.plugin);

    let keys: Vec<&str> = definition
        .object_type
        .properties
        .iter()
        .map(|property| property.key.as_str())
        .collect();
    assert_eq!(
        keys,
        [
            "rating",
            "editable",
            "icon",
            "onChangeAction",
            "columns",
            "Label"
        ]
    );

    let rating = definition.object_type.property("rating").unwrap();
    assert_eq!(rating.value_type.kind, "Attribute");
    assert_eq!(rating.category, "General");
    assert_eq!(rating.caption, "Rating attribute");
    assert!(rating.value_type.required);
    assert_eq!(rating.value_type.on_change_property, "onChangeAction");
    assert_eq!(rating.value_type.attribute_types, ["Integer", "Decimal"]);

    let editable = definition.object_type.property("editable").unwrap();
    assert_eq!(editable.value_type.kind, "Boolean");
    assert_eq!(editable.value_type.default_value, "true");

    let icon = definition.object_type.property("icon").unwrap();
    assert_eq!(icon.category, "General::Appearance");
    assert_eq!(icon.value_type.kind, "Enumeration");
    let enumeration: Vec<(&str, &str)> = icon
        .value_type
        .enumeration_values
        .iter()
        .map(|value| (value.key.as_str(), value.caption.as_str()))
        .collect();
    assert_eq!(enumeration, [("star", "Star"), ("heart", "Heart")]);

    let action = definition.object_type.property("onChangeAction").unwrap();
    assert_eq!(action.category, "Events");
    assert_eq!(action.value_type.kind, "Action");
    assert!(!action.value_type.required);
    assert_eq!(action.value_type.action_variables.len(), 1);
    assert_eq!(action.value_type.action_variables[0].key, "newValue");
    assert_eq!(action.value_type.action_variables[0].kind, "Decimal");
    let return_type = action.value_type.return_type.as_ref().unwrap();
    assert_eq!(return_type.kind, "Boolean");
    assert_eq!(return_type.assignable_to, "editable");

    let columns = definition.object_type.property("columns").unwrap();
    assert_eq!(columns.value_type.kind, "Object");
    assert!(columns.value_type.list);
    let nested = columns.value_type.object_type.as_ref().unwrap();
    // The nested system property is excluded, exactly like mxrb's
    // `nested_properties`.
    assert_eq!(nested.properties.len(), 1);
    let caption = &nested.properties[0];
    assert_eq!(caption.key, "caption");
    assert_eq!(caption.category, "Column");
    assert_eq!(caption.value_type.kind, "TextTemplate");
    assert_eq!(caption.value_type.translations.len(), 1);
    assert_eq!(caption.value_type.translations[0].language, "en_US");
    assert_eq!(caption.value_type.translations[0].text, "Column");

    let label = definition.object_type.property("Label").unwrap();
    assert!(label.is_system());
    assert_eq!(label.caption, "<system:Label>");
    assert_eq!(label.category, "General");
}

#[test]
fn without_a_manifest_every_xml_entry_is_considered() {
    let directory = tempfile::tempdir().unwrap();
    let mpk = directory.path().join("bare.mpk");
    write_mpk(&mpk, &[("Rating.xml", WIDGET_XML)]);
    let definition = mxrs_widget_package::WidgetPackage::new(&mpk)
        .definition("com.example.rating.Rating")
        .unwrap();
    assert!(definition.is_some());
}

#[test]
fn an_unknown_widget_id_is_none_and_an_unknown_property_type_is_loud() {
    let directory = tempfile::tempdir().unwrap();
    let mpk = directory.path().join("bare.mpk");
    write_mpk(&mpk, &[("Rating.xml", WIDGET_XML)]);
    let package = mxrs_widget_package::WidgetPackage::new(&mpk);
    assert!(package.definition("com.example.Other").unwrap().is_none());

    let broken = directory.path().join("broken.mpk");
    write_mpk(
        &broken,
        &[(
            "Widget.xml",
            r#"<widget id="com.example.Bad"><properties>
                 <property key="x" type="hologram"><caption/><description/></property>
               </properties></widget>"#,
        )],
    );
    let error = mxrs_widget_package::WidgetPackage::new(&broken)
        .definition("com.example.Bad")
        .unwrap_err();
    assert!(error.to_string().contains("hologram"), "{error}");
}

#[test]
fn find_scans_widgets_mpks_in_sorted_order_and_skips_unreadable_packages() {
    let directory = tempfile::tempdir().unwrap();
    let widgets = directory.path().join("widgets");
    std::fs::create_dir_all(&widgets).unwrap();
    std::fs::write(widgets.join("aaa.mpk"), "not a zip at all").unwrap();
    write_mpk(
        &widgets.join("bbb.mpk"),
        &[("Other.xml", "<widget id=\"com.example.Other\"/>")],
    );
    write_mpk(&widgets.join("ccc.mpk"), &[("Rating.xml", WIDGET_XML)]);

    let found = mxrs_widget_package::find(directory.path(), "com.example.rating.Rating")
        .unwrap()
        .unwrap();
    assert_eq!(found.name, "Rating");
    assert!(
        mxrs_widget_package::find(directory.path(), "com.example.Missing")
            .unwrap()
            .is_none()
    );
    // No `widgets/` directory at all is simply a miss.
    let empty = tempfile::tempdir().unwrap();
    assert!(
        mxrs_widget_package::find(empty.path(), "com.example.rating.Rating")
            .unwrap()
            .is_none()
    );
    // A matching widget whose schema cannot be represented is loud, even
    // through `find`.
    write_mpk(
        &widgets.join("ddd.mpk"),
        &[(
            "Bad.xml",
            r#"<widget id="com.example.Bad"><properties>
                 <property key="x" type="hologram"><caption/><description/></property>
               </properties></widget>"#,
        )],
    );
    let error = mxrs_widget_package::find(directory.path(), "com.example.Bad").unwrap_err();
    assert!(error.to_string().contains("hologram"), "{error}");
}
