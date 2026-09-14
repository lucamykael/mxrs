//! Real 11.12.1 TextBox.Placeholder is Texts$Text even though the editor
//! schema describes a ClientTemplate. These synthetic documents reproduce
//! that public storage shape without including private corpus content.

use std::rc::Rc;

use mxrs_bson::{Bson, Document, doc};
use mxrs_forms::{
    Catalog, FormsError, MprCodec, Node,
    node::Value,
    values::{Text, Translation},
};

fn codec() -> MprCodec {
    MprCodec::new(Rc::new(Catalog::for_version("11.12.1").unwrap()))
}

fn translation(language: &str, text: &str, id: &str) -> Bson {
    Bson::Document(doc! {
        "Text": text, "LanguageCode": language,
        "$ID": id, "$Type": "Texts$Translation",
    })
}

fn fixture() -> Document {
    doc! {
        "$ID": "60000000-0000-0000-0000-000000000000",
        "$Type": "Forms$TextBox",
        "Name": "search",
        "Placeholder": {
            "Items": [3,
                translation("pt_BR", "Pesquisar", "60000000-0000-0000-0000-000000000001"),
                translation("en_US", "Search", "60000000-0000-0000-0000-000000000002"),
                translation("pt_BR", "Pesquisa alternativa", "60000000-0000-0000-0000-000000000003"),
            ],
            "$Type": "Texts$Text",
            "$ID": "60000000-0000-0000-0000-000000000004",
        },
    }
}

fn placeholder(node: &Node) -> Node {
    let Some(Value::Node(template)) = node.fetch("placeholderTemplate").unwrap() else {
        panic!("translated placeholder must be a typed ClientTemplate");
    };
    template.clone()
}

#[test]
fn legacy_placeholder_preserves_all_translations_and_exact_storage_bytes() {
    let codec = codec();
    let document = fixture();
    let node = codec.decode(&document).unwrap();
    let template = placeholder(&node);
    assert_eq!(template.schema_type().name, "ClientTemplate");
    let Some(Value::Text(text)) = template.fetch("template").unwrap() else {
        panic!("placeholder must expose typed translated text");
    };
    assert_eq!(text.translations.len(), 3);
    assert_eq!(text.translations[0].language.as_deref(), Some("pt_BR"));
    assert_eq!(text.translations[1].text, "Search");
    assert_eq!(text.translations[2].text, "Pesquisa alternativa");
    let encoded = codec.encode(&node).unwrap();
    assert_eq!(
        mxrs_bson::serialize(encoded.get_document("Placeholder").unwrap()).unwrap(),
        mxrs_bson::serialize(document.get_document("Placeholder").unwrap()).unwrap(),
    );
    assert_eq!(codec.decode(&encoded).unwrap(), node);
}

#[test]
fn edits_reordering_and_deletion_preserve_surviving_translation_ids_and_field_order() {
    let codec = codec();
    let document = fixture();
    let mut node = codec.decode(&document).unwrap();
    let mut template = placeholder(&node);
    template
        .set(
            "template",
            Value::Text(Text::from_translations(vec![
                Translation {
                    language: Some("en_US".into()),
                    text: "Find".into(),
                },
                Translation {
                    language: Some("pt_BR".into()),
                    text: "Buscar".into(),
                },
                Translation {
                    language: None,
                    text: "Default".into(),
                },
            ])),
        )
        .unwrap();
    node.set("placeholderTemplate", Value::Node(template))
        .unwrap();
    let encoded = codec.encode(&node).unwrap();
    let text = encoded.get_document("Placeholder").unwrap();
    assert_eq!(
        text.keys().collect::<Vec<_>>(),
        document
            .get_document("Placeholder")
            .unwrap()
            .keys()
            .collect::<Vec<_>>()
    );
    let items = text.get_array("Items").unwrap();
    assert_eq!(items[0], Bson::Int32(3));
    let english = items[1].as_document().unwrap();
    let portuguese = items[2].as_document().unwrap();
    assert_eq!(
        english.get_str("$ID").unwrap(),
        "60000000-0000-0000-0000-000000000002"
    );
    assert_eq!(
        portuguese.get_str("$ID").unwrap(),
        "60000000-0000-0000-0000-000000000001"
    );
    assert_eq!(
        english.keys().map(String::as_str).collect::<Vec<_>>(),
        ["Text", "LanguageCode", "$ID", "$Type"]
    );
    assert_eq!(english.get_str("Text").unwrap(), "Find");
    let added = items[3].as_document().unwrap();
    assert!(!added.contains_key("LanguageCode"));
    assert!(uuid::Uuid::parse_str(added.get_str("$ID").unwrap()).is_ok());
    assert_eq!(codec.decode(&encoded).unwrap(), node);
}

#[test]
fn newly_added_languages_and_duplicate_languages_retain_their_own_values() {
    let codec = codec();
    let mut node = codec.decode(&fixture()).unwrap();
    let mut template = placeholder(&node);
    template
        .set(
            "template",
            Value::Text(Text::from_translations(vec![
                Translation {
                    language: Some("pt_BR".into()),
                    text: "Um".into(),
                },
                Translation {
                    language: Some("pt_BR".into()),
                    text: "Dois".into(),
                },
                Translation {
                    language: Some("fr_FR".into()),
                    text: "Trois".into(),
                },
            ])),
        )
        .unwrap();
    node.set("placeholderTemplate", Value::Node(template))
        .unwrap();
    let encoded = codec.encode(&node).unwrap();
    let items = encoded
        .get_document("Placeholder")
        .unwrap()
        .get_array("Items")
        .unwrap();
    assert_eq!(
        items[2].as_document().unwrap().get_str("$ID").unwrap(),
        "60000000-0000-0000-0000-000000000003"
    );
    assert_eq!(
        items[3]
            .as_document()
            .unwrap()
            .get_str("LanguageCode")
            .unwrap(),
        "fr_FR"
    );
    assert_eq!(codec.decode(&encoded).unwrap(), node);
}

#[test]
fn template_parameters_or_fallback_edits_cannot_be_silently_discarded() {
    let codec = codec();
    let node = codec.decode(&fixture()).unwrap();
    let catalog = node.catalog().clone();
    for (property, value) in [
        ("fallback", Value::Text(Text::from_plain("keep me"))),
        (
            "parameters",
            Value::List(vec![Value::Node(
                Node::new("ClientTemplateParameter", catalog).unwrap(),
            )]),
        ),
    ] {
        let mut changed = node.clone();
        let mut template = placeholder(&changed);
        template.set(property, value).unwrap();
        changed
            .set("placeholderTemplate", Value::Node(template))
            .unwrap();
        let error = codec.encode(&changed).unwrap_err();
        let FormsError::UnrepresentablePlaceholder { path } = error else {
            panic!("expected an explicit lossy-projection error, got {error}");
        };
        assert_eq!(path, "$.Placeholder");
    }
}

#[test]
fn removing_the_required_template_is_reported_without_writing_an_empty_text() {
    let codec = codec();
    let mut node = codec.decode(&fixture()).unwrap();
    let mut template = placeholder(&node);
    template.unset("template").unwrap();
    node.set("placeholderTemplate", Value::Node(template))
        .unwrap();
    assert!(matches!(
        codec.encode(&node),
        Err(FormsError::InvalidShape {
            shape: "Placeholder.template",
            ..
        })
    ));
}

#[test]
fn empty_text_is_byte_exact_and_an_empty_translation_edit_is_not_lost() {
    let codec = codec();
    let mut document = fixture();
    document
        .get_document_mut("Placeholder")
        .unwrap()
        .insert("Items", Vec::<Bson>::new());
    let mut node = codec.decode(&document).unwrap();
    assert_eq!(
        codec.encode(&node).unwrap().get("Placeholder"),
        document.get("Placeholder")
    );
    let mut template = placeholder(&node);
    template
        .set("template", Value::Text(Text::from_plain("")))
        .unwrap();
    node.set("placeholderTemplate", Value::Node(template))
        .unwrap();
    assert_eq!(codec.decode(&codec.encode(&node).unwrap()).unwrap(), node);
}

#[test]
fn text_is_not_accepted_as_an_arbitrary_forms_element() {
    let codec = codec();
    let mut document = fixture();
    let text = document.remove("Placeholder").unwrap();
    document.insert("ScreenReaderLabel", text);
    assert!(matches!(
        codec.decode(&document),
        Err(FormsError::NotAFormsStorageType(_))
    ));
}

#[test]
fn malformed_translation_entries_fail_with_the_placeholder_path() {
    let codec = codec();
    let mut document = fixture();
    document
        .get_document_mut("Placeholder")
        .unwrap()
        .insert("Items", vec![Bson::Int32(3), Bson::Boolean(false)]);
    let FormsError::InvalidShape { shape, path } = codec.decode(&document).unwrap_err() else {
        panic!("malformed translations must fail");
    };
    assert_eq!(shape, "Text.Items[]");
    assert_eq!(path, "$.Placeholder");
}

#[test]
fn malformed_translation_values_cannot_be_normalized_to_empty_strings() {
    let codec = codec();
    for field in ["Text", "LanguageCode"] {
        let mut document = fixture();
        document
            .get_document_mut("Placeholder")
            .unwrap()
            .get_array_mut("Items")
            .unwrap()[1]
            .as_document_mut()
            .unwrap()
            .insert(field, 42);
        let FormsError::InvalidShape { shape, path } = codec.decode(&document).unwrap_err() else {
            panic!("malformed translated strings must fail");
        };
        assert_eq!(shape, "translation string");
        assert_eq!(path, format!("$.Placeholder.Items[0].{field}"));
    }
    let mut document = fixture();
    document
        .get_document_mut("Placeholder")
        .unwrap()
        .insert("Items", false);
    assert!(matches!(
        codec.decode(&document),
        Err(FormsError::InvalidShape {
            shape: "Text.Items",
            ..
        })
    ));
}

#[test]
fn absent_items_and_nullable_languages_preserve_the_original_document() {
    let codec = codec();
    let mut document = fixture();
    document
        .get_document_mut("Placeholder")
        .unwrap()
        .get_array_mut("Items")
        .unwrap()[1]
        .as_document_mut()
        .unwrap()
        .insert("LanguageCode", Bson::Null);
    let node = codec.decode(&document).unwrap();
    assert_eq!(
        codec.encode(&node).unwrap().get("Placeholder"),
        document.get("Placeholder")
    );
    document
        .get_document_mut("Placeholder")
        .unwrap()
        .remove("Items");
    let node = codec.decode(&document).unwrap();
    assert_eq!(
        codec.encode(&node).unwrap().get("Placeholder"),
        document.get("Placeholder")
    );
}
