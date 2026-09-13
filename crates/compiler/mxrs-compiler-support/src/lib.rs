//! Common mechanics shared by the domain, flow, and widget compilers.

use mxrs_bson::{Bson, Document};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

pub fn get_any<'a>(doc: &'a Document, keys: &[&str]) -> Option<&'a Bson> {
    keys.iter().find_map(|key| doc.get(*key))
}

pub fn get(doc: &Document, key: &str) -> Bson {
    doc.get(key).cloned().unwrap_or(Bson::Null)
}

pub fn get_str_any(doc: &Document, keys: &[&str]) -> Option<String> {
    match get_any(doc, keys)? {
        Bson::String(value) => Some(value.clone()),
        _ => None,
    }
}

pub fn get_id_any(doc: &Document, keys: &[&str]) -> Option<String> {
    get_any(doc, keys).and_then(mxrs_bson::extract_id)
}

pub fn get_bool_any(doc: &Document, keys: &[&str]) -> Option<bool> {
    match get_any(doc, keys)? {
        Bson::Boolean(value) => Some(*value),
        _ => None,
    }
}

pub fn get_doc_any(doc: &Document, keys: &[&str]) -> Option<Document> {
    match get_any(doc, keys)? {
        Bson::Document(value) => Some(value.clone()),
        _ => None,
    }
}

pub fn to_s(value: &Bson) -> String {
    match value {
        Bson::Null => String::new(),
        Bson::String(value) => value.clone(),
        other => other.to_string(),
    }
}

pub fn array_items(doc: &Document, key: &str) -> Vec<Bson> {
    array_items_any(doc, &[key])
}

pub fn array_items_any(doc: &Document, keys: &[&str]) -> Vec<Bson> {
    match get_any(doc, keys) {
        Some(Bson::Array(items)) => mxrs_bson::parse_array(Some(items)).items,
        _ => Vec::new(),
    }
}

pub fn array_docs(doc: &Document, keys: &[&str]) -> Vec<Document> {
    array_items_any(doc, keys)
        .into_iter()
        .filter_map(|value| match value {
            Bson::Document(document) => Some(document),
            _ => None,
        })
        .collect()
}

pub fn string_list(doc: &Document, keys: &[&str]) -> Vec<String> {
    array_items_any(doc, keys)
        .into_iter()
        .filter_map(|value| match value {
            Bson::String(item) => Some(item),
            _ => None,
        })
        .collect()
}

pub fn build_array(items: Vec<Bson>) -> Bson {
    Bson::Array(mxrs_bson::build_array(items, 3))
}

pub fn plain_array_field(doc: &Document, key: &str) -> Bson {
    match doc.get(key) {
        None | Some(Bson::Null) => Bson::Array(vec![]),
        Some(_) => build_array(
            array_items(doc, key)
                .into_iter()
                .map(marked_plain_value)
                .collect(),
        ),
    }
}

pub fn plain_document_field(doc: &Document, key: &str) -> Bson {
    match doc.get(key) {
        None | Some(Bson::Null) => Bson::Null,
        Some(value) => marked_plain_value(value.clone()),
    }
}

pub fn plain_value(value: Bson) -> Bson {
    match value {
        Bson::Document(document) => Bson::Document(
            document
                .into_iter()
                .map(|(key, value)| (key, plain_value(value)))
                .collect(),
        ),
        Bson::Array(items) => Bson::Array(
            mxrs_bson::parse_array(Some(&items))
                .items
                .into_iter()
                .map(plain_value)
                .collect(),
        ),
        other => other,
    }
}

fn marked_plain_value(value: Bson) -> Bson {
    match value {
        Bson::Document(document) => Bson::Document(
            document
                .into_iter()
                .map(|(key, value)| (key, marked_plain_value(value)))
                .collect(),
        ),
        Bson::Array(items) => build_array(
            mxrs_bson::parse_array(Some(&items))
                .items
                .into_iter()
                .map(marked_plain_value)
                .collect(),
        ),
        other => other,
    }
}

pub fn stable_dedup(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.clone()))
        .collect()
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn project_role_map(
    project: &mxrs_model::Project,
) -> mxrs_model::Result<HashMap<String, Vec<String>>> {
    let mut map = HashMap::new();
    let units = project.all_units()?;
    let security = units.iter().find_map(|unit| {
        let document = project.mpr().parse_contents(unit).ok()?;
        (get_str_any(&document, &["$Type"]).as_deref() == Some("Security$ProjectSecurity"))
            .then_some(document)
    });
    let Some(security) = security else {
        return Ok(map);
    };
    for role in array_docs(&security, &["UserRoles"]) {
        let name = get_str_any(&role, &["Name"]).unwrap_or_default();
        for module_role in string_list(&role, &["ModuleRoles"]) {
            map.entry(module_role)
                .or_insert_with(Vec::new)
                .push(name.clone());
        }
    }
    Ok(map)
}

pub fn runtime_data_type(document: Option<&Document>) -> Result<String, String> {
    let Some(document) = document else {
        return Ok("Void".to_string());
    };
    let type_name = get_str_any(document, &["$Type"]).unwrap_or_default();
    Ok(match type_name.as_str() {
        "DataTypes$VoidType" => "Void".to_string(),
        "DataTypes$StringType" => "String".to_string(),
        "DataTypes$BooleanType" => "Boolean".to_string(),
        "DataTypes$IntegerType" => "Integer".to_string(),
        "DataTypes$DecimalType" => "Decimal".to_string(),
        "DataTypes$DateTimeType" => "DateTime".to_string(),
        "DataTypes$ObjectType" => get_str_any(document, &["Entity"]).unwrap_or_default(),
        "DataTypes$ListType" => format!(
            "[{}]",
            get_str_any(document, &["Entity"]).unwrap_or_default()
        ),
        "DataTypes$EnumerationType" => format!(
            "#{}",
            get_str_any(document, &["Enumeration"]).unwrap_or_default()
        ),
        "DataTypes$UnknownType" => "Unknown".to_string(),
        other => return Err(other.to_string()),
    })
}

pub fn derived_id(source_id: &str, label: &str) -> String {
    derived_id_from_seed(&format!("{source_id}:{label}"))
}

pub fn database_derived_id(source_id: &str, label: &str) -> String {
    derived_id_from_seed(&format!("{source_id}:database-connector:{label}"))
}

/// Stable Runtime operation identity shared by web catalogs and nanoflow
/// client instructions.
pub fn operation_id(page_name: &str, widget_name: &str) -> String {
    let digest = Sha256::digest(format!("{page_name}/{widget_name}").as_bytes());
    base64_standard(&digest[..16])
        .trim_end_matches("==")
        .to_string()
}

/// Stable client-side list-property id used by Data Grid 2, Gallery, and
/// Combo Box. Matches mxrb's first 24 bits of SHA-256 interpreted as hex.
pub fn widget_data_source_id(widget_key: &str) -> String {
    let digest = Sha256::digest(widget_key.as_bytes());
    let value = (u32::from(digest[0]) << 16) | (u32::from(digest[1]) << 8) | u32::from(digest[2]);
    format!("p.{value}")
}

pub fn menu_operation_id(action: &Document) -> String {
    let identifier = action
        .get("$ID")
        .and_then(mxrs_bson::extract_id)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            get_doc_any(action, &["MicroflowSettings"])
                .and_then(|settings| get_str_any(&settings, &["Microflow"]))
        })
        .unwrap_or_default();
    operation_id("Navigation", &identifier)
}

fn base64_standard(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::new();
    for chunk in bytes.chunks(3) {
        let first = u32::from(chunk[0]);
        let second = u32::from(*chunk.get(1).unwrap_or(&0));
        let third = u32::from(*chunk.get(2).unwrap_or(&0));
        let packed = (first << 16) | (second << 8) | third;
        output.push(ALPHABET[((packed >> 18) & 0x3f) as usize] as char);
        output.push(ALPHABET[((packed >> 12) & 0x3f) as usize] as char);
        output.push(if chunk.len() > 1 {
            ALPHABET[((packed >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[(packed & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    output
}

fn derived_id_from_seed(seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    let hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let hex = &hex[..32];
    format!(
        "{}-{}-4{}-8{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_ids_are_deterministic_and_namespaced() {
        let plain = derived_id("00000000-0000-0000-0000-000000000000", "label");
        assert_eq!(
            plain,
            derived_id("00000000-0000-0000-0000-000000000000", "label")
        );
        assert_eq!(plain.len(), 36);
        assert_eq!(plain.chars().nth(14), Some('4'));
        assert_eq!(plain.chars().nth(19), Some('8'));
        assert_ne!(
            plain,
            database_derived_id("00000000-0000-0000-0000-000000000000", "label")
        );
    }

    #[test]
    fn plain_value_strips_nested_array_markers() {
        let value = Bson::Array(mxrs_bson::build_array(
            vec![Bson::Array(mxrs_bson::build_array(
                vec![Bson::String("x".into())],
                2,
            ))],
            2,
        ));
        let Bson::Array(outer) = plain_value(value) else {
            panic!("expected array");
        };
        assert_eq!(outer.len(), 1);
        let Bson::Array(inner) = &outer[0] else {
            panic!("expected nested array");
        };
        assert_eq!(inner, &[Bson::String("x".into())]);
    }

    #[test]
    fn operation_ids_match_the_ruby_sha256_base64_contract() {
        assert_eq!(
            operation_id("Sales.MyPage", "widget1"),
            "Xt1yRHfyEC1EqtmPr5GL8Q"
        );
        let action = mxrs_bson::doc! {
            "MicroflowSettings": { "Microflow": "Sales.ACT_Run" }
        };
        assert_eq!(
            menu_operation_id(&action),
            operation_id("Navigation", "Sales.ACT_Run")
        );
    }

    #[test]
    fn widget_data_source_ids_match_the_first_six_sha256_hex_digits() {
        let key = "p.Sales.Orders.gallery";
        let digest = Sha256::digest(key.as_bytes());
        let expected = u32::from_str_radix(
            &format!("{:02x}{:02x}{:02x}", digest[0], digest[1], digest[2]),
            16,
        )
        .unwrap();
        assert_eq!(widget_data_source_id(key), format!("p.{expected}"));
    }
}
