//! Ports `lib/mxrb/compiler/artifact_document_compiler.rb` (102 lines): a
//! pure, stateless dispatch over Runtime roots whose executable
//! representation is independent of UI bundles. Unlike
//! [`crate::navigation::NavigationCompiler`]/[`crate::settings::SettingsCompiler`],
//! this compiler has **no** `RuntimeModelSchema` dependency — mxrb's own
//! version never calls `@schema.fields_for` here either; every output
//! field is a hardcoded literal set, so `ArtifactCompiler` is a plain
//! stateless dispatcher (associated functions, no `&self`), matching
//! `mxrs-compiler-flow::CodeActionCompiler`'s own precedent for a
//! compiler with no cross-call state to hold.

use mxrs_bson::{Bson, Document, doc};

use crate::CompilerError;
use crate::image_format::image_format;
use crate::support::{array_items, build_array, get, plain_document_field, to_s};

/// `$Type`s compiled via the shallow `{$ID, $Type, Name, QualifiedName}`
/// shape ([`named`]) — mirrors mxrb's `NAME_ONLY_TYPES`.
pub const NAME_ONLY_TYPES: &[&str] = &[
    "CustomIcons$CustomIconCollection",
    "Forms$Layout",
    "Forms$Snippet",
    "Menus$MenuDocument",
];

/// Every `$Type` this compiler handles — mirrors mxrb's `TYPES`
/// (`NAME_ONLY_TYPES` plus the four types with their own compile method,
/// one of which — `DomainModels$ViewEntitySourceDocument` — also ends up
/// routed to the shallow `named` shape; see [`ArtifactCompiler::compile`]).
pub const TYPES: &[&str] = &[
    "CustomIcons$CustomIconCollection",
    "Forms$Layout",
    "Forms$Snippet",
    "Menus$MenuDocument",
    "Enumerations$Enumeration",
    "Images$ImageCollection",
    "RegularExpressions$RegularExpression",
    "ScheduledEvents$ScheduledEvent",
    "DomainModels$ViewEntitySourceDocument",
];

pub struct ArtifactCompiler;

impl ArtifactCompiler {
    pub fn compile(
        source: &Document,
        module_name: Option<&str>,
    ) -> Result<Document, CompilerError> {
        let type_name = source.get_str("$Type").unwrap_or_default();
        if NAME_ONLY_TYPES.contains(&type_name) {
            return named(source, module_name);
        }
        match type_name {
            "Enumerations$Enumeration" => enumeration(source, module_name),
            "Images$ImageCollection" => image_collection(source, module_name),
            "RegularExpressions$RegularExpression" => regular_expression(source, module_name),
            "ScheduledEvents$ScheduledEvent" => scheduled_event(source, module_name),
            "DomainModels$ViewEntitySourceDocument" => named(source, module_name),
            other => Err(CompilerError::UnsupportedArtifactType {
                type_name: other.to_string(),
            }),
        }
    }
}

fn qualified_name(module_name: Option<&str>, name: &Bson) -> Result<String, CompilerError> {
    let module_name =
        module_name.ok_or_else(|| CompilerError::ArtifactOutsideModule { name: to_s(name) })?;
    Ok(format!("{module_name}.{}", to_s(name)))
}

fn named(source: &Document, module_name: Option<&str>) -> Result<Document, CompilerError> {
    let name = get(source, "Name");
    let qualified = qualified_name(module_name, &name)?;
    Ok(doc! {
        "$ID": get(source, "$ID"),
        "$Type": get(source, "$Type"),
        "Name": name,
        "QualifiedName": qualified,
    })
}

fn enumeration(source: &Document, module_name: Option<&str>) -> Result<Document, CompilerError> {
    let name = get(source, "Name");
    let qualified = qualified_name(module_name, &name)?;
    let values = array_items(source, "Values")
        .into_iter()
        .filter_map(|v| v.as_document().cloned())
        .map(|v| Bson::Document(enumeration_value(&v)))
        .collect();
    Ok(doc! {
        "$ID": get(source, "$ID"),
        "$Type": get(source, "$Type"),
        "Values": build_array(values),
        "Name": name,
        "QualifiedName": qualified,
    })
}

fn enumeration_value(source: &Document) -> Document {
    doc! {
        "$ID": get(source, "$ID"),
        "$Type": get(source, "$Type"),
        "Caption": text_reference(source.get_document("Caption").ok()),
        "RemoteValue": get(source, "RemoteValue"),
        "Name": get(source, "Name"),
        "Image": to_s(&get(source, "Image")),
    }
}

fn image_collection(
    source: &Document,
    module_name: Option<&str>,
) -> Result<Document, CompilerError> {
    let name = get(source, "Name");
    let collection_name = qualified_name(module_name, &name)?;
    let images = array_items(source, "Images")
        .into_iter()
        .filter_map(|v| v.as_document().cloned())
        .map(|v| image(&v, &collection_name).map(Bson::Document))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(doc! {
        "$ID": get(source, "$ID"),
        "$Type": get(source, "$Type"),
        "Images": build_array(images),
        "Name": name,
        "QualifiedName": collection_name,
    })
}

fn image(source: &Document, collection_name: &str) -> Result<Document, CompilerError> {
    let format = image_format(source)?;
    Ok(doc! {
        "$ID": get(source, "$ID"),
        "$Type": get(source, "$Type"),
        "Name": get(source, "Name"),
        "Image": get(source, "Image"),
        "Format": format,
        "QualifiedName": format!("{collection_name}.{}", to_s(&get(source, "Name"))),
    })
}

fn regular_expression(
    source: &Document,
    module_name: Option<&str>,
) -> Result<Document, CompilerError> {
    let mut result = named(source, module_name)?;
    result.insert("Expression", to_s(&get(source, "Expression")));
    Ok(result)
}

fn scheduled_event(
    source: &Document,
    module_name: Option<&str>,
) -> Result<Document, CompilerError> {
    let name = get(source, "Name");
    let qualified = qualified_name(module_name, &name)?;
    Ok(doc! {
        "$ID": get(source, "$ID"),
        "$Type": get(source, "$Type"),
        "Schedule": plain_document_field(source, "Schedule"),
        "Name": name,
        "QualifiedName": qualified,
        "StartDateTime": get(source, "StartDateTime"),
        "TimeZone": get(source, "TimeZone"),
        "OnOverlap": get(source, "OnOverlap"),
        "Interval": get(source, "Interval"),
        "IntervalType": get(source, "IntervalType"),
        "Microflow": get(source, "Microflow"),
        "Documentation": to_s(&get(source, "Documentation")),
    })
}

/// Shallow `{$ID, $Type}` reference stub — distinct from
/// [`crate::navigation`]'s deep/recursive `text_reference` (same name,
/// deliberately different behavior, matching mxrb's own two separate
/// private methods of the same name in two different compiler classes).
fn text_reference(source: Option<&Document>) -> Bson {
    match source {
        None => Bson::Null,
        Some(source) => Bson::Document(doc! {
            "$ID": get(source, "$ID"),
            "$Type": get(source, "$Type"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_bson::{Binary, BinarySubtype};

    #[test]
    fn name_only_types_compile_to_the_shallow_shape() {
        let source = doc! {
            "$ID": "11111111-1111-4111-8111-111111111111",
            "$Type": "Forms$Layout",
            "Name": "MyLayout",
        };
        let compiled = ArtifactCompiler::compile(&source, Some("Sales")).unwrap();
        assert_eq!(compiled.get_str("QualifiedName").unwrap(), "Sales.MyLayout");
        assert_eq!(compiled.len(), 4);
    }

    #[test]
    fn view_entity_source_document_uses_the_shallow_shape_too() {
        let source = doc! {
            "$ID": "11111111-1111-4111-8111-111111111111",
            "$Type": "DomainModels$ViewEntitySourceDocument",
            "Name": "View",
        };
        let compiled = ArtifactCompiler::compile(&source, Some("App")).unwrap();
        assert_eq!(compiled.get_str("QualifiedName").unwrap(), "App.View");
    }

    #[test]
    fn a_name_only_artifact_outside_a_module_is_a_loud_error() {
        let source = doc! {
            "$ID": "11111111-1111-4111-8111-111111111111",
            "$Type": "Forms$Layout",
            "Name": "MyLayout",
        };
        let err = ArtifactCompiler::compile(&source, None).unwrap_err();
        assert!(matches!(err, CompilerError::ArtifactOutsideModule { name } if name == "MyLayout"));
    }

    #[test]
    fn an_unsupported_type_is_a_loud_error() {
        let source = doc! {
            "$ID": "11111111-1111-4111-8111-111111111111",
            "$Type": "Something$Unknown",
        };
        let err = ArtifactCompiler::compile(&source, Some("App")).unwrap_err();
        assert!(
            matches!(err, CompilerError::UnsupportedArtifactType { type_name } if type_name == "Something$Unknown")
        );
    }

    #[test]
    fn enumeration_value_image_defaults_to_empty_string() {
        let source = doc! {
            "$ID": "11111111-1111-4111-8111-111111111111",
            "$Type": "Enumerations$EnumerationValue",
            "Name": "Yes",
        };
        let value = enumeration_value(&source);
        assert_eq!(value.get_str("Image").unwrap(), "");
        assert!(matches!(value.get("Caption"), Some(Bson::Null)));
    }

    #[test]
    fn enumeration_compiles_its_values_and_strips_the_array_marker() {
        let source = doc! {
            "$ID": "11111111-1111-4111-8111-111111111111",
            "$Type": "Enumerations$Enumeration",
            "Name": "Status",
            "Values": mxrs_bson::build_array(
                vec![Bson::Document(doc! {
                    "$ID": "22222222-2222-4222-8222-222222222222",
                    "$Type": "Enumerations$EnumerationValue",
                    "Name": "Open",
                })],
                3,
            ),
        };
        let compiled = ArtifactCompiler::compile(&source, Some("Sales")).unwrap();
        assert_eq!(compiled.get_str("QualifiedName").unwrap(), "Sales.Status");
        let Some(Bson::Array(values)) = compiled.get("Values") else {
            panic!("expected an array");
        };
        // marker int + one compiled value
        assert_eq!(values.len(), 2);
    }

    #[test]
    fn image_collection_compiles_dotted_qualified_names_and_sniffs_format() {
        let source = doc! {
            "$ID": "11111111-1111-4111-8111-111111111111",
            "$Type": "Images$ImageCollection",
            "Name": "Icons",
            "Images": mxrs_bson::build_array(
                vec![Bson::Document(doc! {
                    "$ID": "22222222-2222-4222-8222-222222222222",
                    "$Type": "Images$Image",
                    "Name": "Logo",
                    "Image": Binary { subtype: BinarySubtype::Generic, bytes: b"\x89PNG\r\n\x1A\nrest".to_vec() },
                })],
                3,
            ),
        };
        let compiled = ArtifactCompiler::compile(&source, Some("App")).unwrap();
        let Some(Bson::Array(images)) = compiled.get("Images") else {
            panic!("expected an array");
        };
        let Bson::Document(image) = &images[1] else {
            panic!("expected a document");
        };
        assert_eq!(image.get_str("Format").unwrap(), "png");
        assert_eq!(image.get_str("QualifiedName").unwrap(), "App.Icons.Logo");
    }

    #[test]
    fn regular_expression_merges_the_expression_field() {
        let source = doc! {
            "$ID": "11111111-1111-4111-8111-111111111111",
            "$Type": "RegularExpressions$RegularExpression",
            "Name": "Email",
            "Expression": "^.+@.+$",
        };
        let compiled = ArtifactCompiler::compile(&source, Some("App")).unwrap();
        assert_eq!(compiled.get_str("Expression").unwrap(), "^.+@.+$");
        assert_eq!(compiled.get_str("QualifiedName").unwrap(), "App.Email");
    }

    #[test]
    fn scheduled_event_defaults_documentation_to_empty_string() {
        let source = doc! {
            "$ID": "11111111-1111-4111-8111-111111111111",
            "$Type": "ScheduledEvents$ScheduledEvent",
            "Name": "Nightly",
        };
        let compiled = ArtifactCompiler::compile(&source, Some("App")).unwrap();
        assert_eq!(compiled.get_str("Documentation").unwrap(), "");
        assert_eq!(compiled.get_str("QualifiedName").unwrap(), "App.Nightly");
    }
}
