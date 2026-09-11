//! Lossless BSON codec for the typed Studio Pro 11 project-settings model.
//!
//! Ports `Mxrb::Settings::MprCodec` from `lib/mxrb/settings/mpr_codec.rb`.
//! `encode`'s `baseline` parameter matters for round-trip stability: when a
//! node/collection item matches something in the previous document (by
//! `$Type` + an identity field, or failing that by position), its `$ID` is
//! reused verbatim; only genuinely new items get a fresh id — and even that
//! id is a deterministic hash of the item's path, not random, so re-encoding
//! the exact same tree twice (with or without a baseline) always produces
//! byte-identical output.

use mxrs_bson::{Bson, Document};
use sha2::{Digest, Sha256};

use crate::error::{Result, SettingsError};
use crate::node::Node;
use crate::value::{BinaryAsset, Collection, Value};

pub fn decode(document: &Document) -> Result<Node> {
    let node = decode_node(document)?;
    if node.storage_type() != "Settings$ProjectSettings" {
        return Err(SettingsError::InvalidRoot);
    }
    Ok(node)
}

fn decode_value(value: &Bson) -> Result<Value> {
    match value {
        Bson::Binary(b) => Ok(Value::Binary(BinaryAsset::from_bytes(
            b.bytes.clone(),
            b.subtype,
        ))),
        Bson::Array(items) => decode_collection(items),
        Bson::Document(doc) => Ok(Value::Node(decode_node(doc)?)),
        Bson::String(s) => Ok(Value::String(s.clone())),
        Bson::Int32(i) => Ok(Value::Integer(i64::from(*i))),
        Bson::Int64(i) => Ok(Value::Integer(*i)),
        Bson::Double(f) => Ok(Value::Float(*f)),
        Bson::Boolean(b) => Ok(Value::Boolean(*b)),
        Bson::DateTime(dt) => Ok(Value::Time(*dt)),
        Bson::Null => Ok(Value::Null),
        other => Err(SettingsError::UnsupportedBsonValue(format!("{other:?}"))),
    }
}

fn decode_collection(raw: &[Bson]) -> Result<Value> {
    let parsed = mxrs_bson::parse_array(Some(raw));
    let items = parsed
        .items
        .iter()
        .map(decode_value)
        .collect::<Result<Vec<_>>>()?;
    Ok(Value::Collection(Collection::new(items, parsed.marker)?))
}

fn decode_node(document: &Document) -> Result<Node> {
    let type_name = document
        .get_str("$Type")
        .map_err(|_| SettingsError::UntypedMap)?
        .to_string();
    let mut node = Node::new(type_name)?;
    for (field, value) in document {
        if field == "$ID" || field == "$Type" {
            continue;
        }
        node.set(field, decode_value(value)?)?;
    }
    Ok(node)
}

pub fn encode(model: &Node, baseline: Option<&Document>) -> Result<Document> {
    if model.storage_type() != "Settings$ProjectSettings" {
        return Err(SettingsError::InvalidRoot);
    }
    encode_node(model, baseline, &["project_settings".to_string()])
}

fn encode_node(node: &Node, baseline: Option<&Document>, path: &[String]) -> Result<Document> {
    let previous = baseline.filter(|b| {
        b.get_str("$Type")
            .map(|t| t == node.storage_type())
            .unwrap_or(false)
    });

    let mut document = Document::new();
    document.insert("$ID", encoded_identity(previous, path, node.storage_type()));
    document.insert("$Type", node.storage_type());
    for (field, value) in node.fields() {
        let field_path = extend_path(path, std::slice::from_ref(field));
        let previous_field = previous.and_then(|p| p.get(field));
        document.insert(
            field.clone(),
            encode_value(value, previous_field, &field_path)?,
        );
    }
    Ok(document)
}

fn encoded_identity(previous: Option<&Document>, path: &[String], storage_type: &str) -> String {
    let reused = previous
        .and_then(|p| p.get("$ID"))
        .and_then(mxrs_bson::extract_id);
    match reused {
        Some(id) if !id.is_empty() => id,
        _ => stable_id(&extend_path(path, &[storage_type.to_string()])),
    }
}

fn encode_value(value: &Value, baseline: Option<&Bson>, path: &[String]) -> Result<Bson> {
    match value {
        Value::Node(node) => {
            let baseline_doc = baseline.and_then(|b| {
                if let Bson::Document(d) = b {
                    Some(d)
                } else {
                    None
                }
            });
            Ok(Bson::Document(encode_node(node, baseline_doc, path)?))
        }
        Value::Collection(collection) => encode_collection(collection, baseline, path),
        Value::Binary(asset) => Ok(Bson::Binary(mxrs_bson::Binary {
            subtype: asset.subtype,
            bytes: asset.bytes.clone(),
        })),
        Value::Null => Ok(Bson::Null),
        Value::Boolean(b) => Ok(Bson::Boolean(*b)),
        Value::Integer(i) => Ok(Bson::Int64(*i)),
        Value::Float(f) => Ok(Bson::Double(*f)),
        Value::String(s) => Ok(Bson::String(s.clone())),
        Value::Time(dt) => Ok(Bson::DateTime(*dt)),
    }
}

fn encode_collection(
    collection: &Collection,
    baseline: Option<&Bson>,
    path: &[String],
) -> Result<Bson> {
    let previous_items = bson_items(baseline);
    let matches = collection_matches(&collection.items, &previous_items);

    let mut items = Vec::with_capacity(collection.items.len());
    for (index, item) in collection.items.iter().enumerate() {
        let previous_index = matches[index]
            .or_else(|| positional_node_index(item, &previous_items, &matches, index));
        let previous_value = previous_index.map(|i| &previous_items[i]);
        let item_path = extend_path(path, &identity_key_path(index, item));
        items.push(encode_value(item, previous_value, &item_path)?);
    }
    Ok(Bson::Array(mxrs_bson::build_array(
        items,
        collection.marker,
    )))
}

/// For each new item, the index of a matching baseline item — by identity
/// key first, falling back to same-position-and-type in
/// [`positional_node_index`]. Consumed baseline indices are tracked so one
/// baseline item is never reused for two new items.
fn collection_matches(items: &[Value], previous: &[Bson]) -> Vec<Option<usize>> {
    let mut consumed = vec![false; previous.len()];
    let mut matches = Vec::with_capacity(items.len());
    for item in items {
        let Value::Node(node) = item else {
            matches.push(None);
            continue;
        };
        let key = identity_key(node);
        let found = previous.iter().enumerate().find(|(i, value)| {
            if consumed[*i] {
                return false;
            }
            let Bson::Document(doc) = value else {
                return false;
            };
            if doc.get_str("$Type").map(str::to_string).ok().as_deref() != Some(node.storage_type())
            {
                return false;
            }
            match &key {
                Some((field, value)) => doc.get_str(field).ok() == Some(value.as_str()),
                None => true,
            }
        });
        if let Some((i, _)) = found {
            consumed[i] = true;
        }
        matches.push(found.map(|(i, _)| i));
    }
    matches
}

fn positional_node_index(
    item: &Value,
    previous: &[Bson],
    matches: &[Option<usize>],
    index: usize,
) -> Option<usize> {
    let Value::Node(node) = item else { return None };
    if matches.contains(&Some(index)) || index >= previous.len() {
        return None;
    }
    match &previous[index] {
        Bson::Document(doc) => {
            (doc.get_str("$Type").ok() == Some(node.storage_type())).then_some(index)
        }
        _ => None,
    }
}

/// The `(field, value)` used to correlate this node across re-encodes —
/// the first of `Name`/`Code`/`ActionActivityType`/`ModuleName` it has set.
fn identity_key(node: &Node) -> Option<(&'static str, String)> {
    const CANDIDATES: &[&str] = &["Name", "Code", "ActionActivityType", "ModuleName"];
    CANDIDATES.iter().find_map(|field| match node.fetch(field) {
        Ok(Value::String(s)) => Some((*field, s.clone())),
        _ => None,
    })
}

fn identity_key_path(index: usize, item: &Value) -> Vec<String> {
    let mut parts = vec![index.to_string()];
    if let Value::Node(node) = item
        && let Some((field, value)) = identity_key(node)
    {
        parts.push(field.to_string());
        parts.push(value);
    }
    parts
}

fn bson_items(value: Option<&Bson>) -> Vec<Bson> {
    match value {
        Some(Bson::Array(items)) => mxrs_bson::parse_array(Some(items)).items,
        _ => Vec::new(),
    }
}

fn extend_path(path: &[String], suffix: &[String]) -> Vec<String> {
    path.iter().cloned().chain(suffix.iter().cloned()).collect()
}

/// Deterministic, UUID-shaped id derived from `parts`: not randomly
/// generated, so re-encoding the same tree (even with no baseline) always
/// assigns the same ids to genuinely-new nodes.
fn stable_id(parts: &[String]) -> String {
    let joined = parts.join("\0");
    let digest = Sha256::digest(joined.as_bytes());
    let mut hex: Vec<u8> = digest
        .iter()
        .flat_map(|b| format!("{b:02x}").into_bytes())
        .collect();
    hex.truncate(32);
    hex[12] = b'5';
    let nibble = (hex_nibble(hex[16]) & 0x3) | 0x8;
    hex[16] = nibble_to_hex(nibble);
    let hex_str = String::from_utf8(hex).expect("hex digits are always valid UTF-8");
    format!(
        "{}-{}-{}-{}-{}",
        &hex_str[0..8],
        &hex_str[8..12],
        &hex_str[12..16],
        &hex_str[16..20],
        &hex_str[20..32]
    )
}

fn hex_nibble(byte: u8) -> u8 {
    (byte as char)
        .to_digit(16)
        .expect("sha256 hex digest is always valid hex") as u8
}

fn nibble_to_hex(nibble: u8) -> u8 {
    std::char::from_digit(u32::from(nibble), 16).expect("nibble is always < 16") as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_bson::doc;

    fn project_settings(parts: Vec<Bson>) -> Document {
        doc! { "$ID": "root-id", "$Type": "Settings$ProjectSettings", "Settings": mxrs_bson::build_array(parts, 2) }
    }

    #[test]
    fn decode_rejects_non_project_settings_root() {
        let document = doc! { "$ID": "x", "$Type": "Settings$ServerConfiguration" };
        assert!(matches!(decode(&document), Err(SettingsError::InvalidRoot)));
    }

    #[test]
    fn decode_then_encode_with_baseline_round_trips_byte_identical() {
        // Settings$ConventionSettings is a real PART_TYPE, valid directly
        // under the top-level Settings collection.
        let part = Bson::Document(doc! {
            "$ID": "conv-id", "$Type": "Settings$ConventionSettings",
            "DefaultAssociationStorage": "Generalization", "LowerCaseMicroflowVariables": true
        });
        let original = project_settings(vec![part]);

        let model = decode(&original).unwrap();
        let encoded = encode(&model, Some(&original)).unwrap();

        assert_eq!(encoded, original);
    }

    /// `Settings$ServerConfiguration` isn't a PART_TYPE itself — it only
    /// appears nested under `Settings$ConfigurationSettings.Configurations`,
    /// which in turn is one of the real top-level Settings parts. This
    /// fixture mirrors that real nesting.
    fn nested_server_configuration(custom_settings: Vec<Bson>) -> Bson {
        let server = doc! {
            "$ID": "server-id", "$Type": "Settings$ServerConfiguration", "Name": "Default",
            "CustomSettings": mxrs_bson::build_array(custom_settings, 3)
        };
        Bson::Document(doc! {
            "$ID": "config-id", "$Type": "Settings$ConfigurationSettings",
            "Configurations": mxrs_bson::build_array(vec![Bson::Document(server)], 3)
        })
    }

    #[test]
    fn encode_reuses_ids_from_baseline_by_identity_key_even_when_reordered() {
        let a = Bson::Document(
            doc! { "$ID": "id-a", "$Type": "Settings$CustomSetting", "Name": "A", "Value": "1" },
        );
        let b = Bson::Document(
            doc! { "$ID": "id-b", "$Type": "Settings$CustomSetting", "Name": "B", "Value": "2" },
        );
        let original = project_settings(vec![nested_server_configuration(vec![a, b])]);

        let mut model = decode(&original).unwrap();
        // Reorder CustomSettings: B before A, deep inside Settings[0]
        // (ConfigurationSettings) -> Configurations[0] (ServerConfiguration).
        let Value::Collection(top) = model.fields()[0].1.clone() else {
            unreachable!()
        };
        let mut top_items = top.items;
        let Value::Node(config_node) = &mut top_items[0] else {
            unreachable!()
        };
        let Value::Collection(configurations) =
            config_node.fetch("Configurations").unwrap().clone()
        else {
            unreachable!()
        };
        let mut config_items = configurations.items;
        let Value::Node(server_node) = &mut config_items[0] else {
            unreachable!()
        };
        let Value::Collection(custom) = server_node.fetch("CustomSettings").unwrap().clone() else {
            unreachable!()
        };
        let mut reordered = custom.items;
        reordered.swap(0, 1);
        server_node
            .set(
                "CustomSettings",
                Value::Collection(Collection::new(reordered, custom.marker).unwrap()),
            )
            .unwrap();
        config_node
            .set(
                "Configurations",
                Value::Collection(Collection::new(config_items, configurations.marker).unwrap()),
            )
            .unwrap();
        model
            .set(
                "Settings",
                Value::Collection(Collection::new(top_items, top.marker).unwrap()),
            )
            .unwrap();

        let encoded = encode(&model, Some(&original)).unwrap();
        let Bson::Array(settings) = encoded.get("Settings").unwrap() else {
            panic!("expected array")
        };
        let Bson::Document(config_doc) = &settings[1] else {
            panic!("expected doc")
        };
        let Bson::Array(configurations) = config_doc.get("Configurations").unwrap() else {
            panic!("expected array")
        };
        let Bson::Document(server_doc) = &configurations[1] else {
            panic!("expected doc")
        };
        let Bson::Array(custom_settings) = server_doc.get("CustomSettings").unwrap() else {
            panic!("expected array")
        };
        let Bson::Document(first) = &custom_settings[1] else {
            panic!("expected doc")
        };
        let Bson::Document(second) = &custom_settings[2] else {
            panic!("expected doc")
        };
        // B now comes first positionally, but keeps id-b; A keeps id-a.
        assert_eq!(first.get_str("Name").unwrap(), "B");
        assert_eq!(
            mxrs_bson::extract_id(first.get("$ID").unwrap()).unwrap(),
            "id-b"
        );
        assert_eq!(second.get_str("Name").unwrap(), "A");
        assert_eq!(
            mxrs_bson::extract_id(second.get("$ID").unwrap()).unwrap(),
            "id-a"
        );
    }

    #[test]
    fn encode_without_baseline_assigns_deterministic_ids() {
        let mut model = Node::new("Settings$ProjectSettings").unwrap();
        let part = Node::new("Settings$ConventionSettings").unwrap();
        model
            .set(
                "Settings",
                Value::Collection(Collection::new(vec![Value::Node(part)], 2).unwrap()),
            )
            .unwrap();

        let first = encode(&model, None).unwrap();
        let second = encode(&model, None).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn stable_id_has_uuid_shape_with_version_and_variant_nibbles_forced() {
        let id = stable_id(&["a".to_string(), "b".to_string()]);
        assert_eq!(id.len(), 36);
        assert_eq!(id.chars().nth(14), Some('5'));
        assert!(matches!(id.chars().nth(19), Some('8' | '9' | 'a' | 'b')));
    }
}
