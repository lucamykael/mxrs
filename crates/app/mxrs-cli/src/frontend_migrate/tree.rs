//! Reading and comparing the untyped document tree the migrations work on.
//!
//! The migrations compare stored documents the way mxrb's do: two documents
//! are the same when they hold the same keys with the same values in any
//! order, and an integer is the same number whatever its BSON width. A
//! schema comparison also ignores identities (`$ID`, `TypePointer`), which a
//! freshly built schema never shares with the stored one.

use mxrs_bson::{Bson, Document};

/// The keys that only identify a node, ignored when schemas are compared.
const IDENTITY_KEYS: [&str; 2] = ["$ID", "TypePointer"];

/// The items of a Mendix array, without its leading marker. Anything that
/// is not an array holds none.
pub(super) fn items(value: Option<&Bson>) -> &[Bson] {
    match value {
        Some(Bson::Array(raw)) => match raw.first() {
            Some(first) if is_integer(first) => &raw[1..],
            _ => raw,
        },
        _ => &[],
    }
}

/// The items of a Mendix array, without its leading marker, to change in
/// place.
pub(super) fn items_mut(value: Option<&mut Bson>) -> &mut [Bson] {
    match value {
        Some(Bson::Array(raw)) => match raw.first() {
            Some(first) if is_integer(first) => &mut raw[1..],
            _ => raw,
        },
        _ => &mut [],
    }
}

/// The marker of a Mendix array; `3` when it has none.
pub(super) fn marker(value: Option<&Bson>) -> i32 {
    match value {
        Some(Bson::Array(raw)) => match raw.first() {
            Some(Bson::Int32(marker)) => *marker,
            Some(Bson::Int64(marker)) => i32::try_from(*marker).unwrap_or(3),
            _ => 3,
        },
        _ => 3,
    }
}

/// A Mendix array of `items` behind `marker`.
pub(super) fn array(items: Vec<Bson>, marker: i32) -> Bson {
    Bson::Array(mxrs_bson::build_array(items, marker))
}

/// A field of a document; nothing for any other node.
pub(super) fn field<'value>(value: &'value Bson, key: &str) -> Option<&'value Bson> {
    match value {
        Bson::Document(document) => document.get(key),
        _ => None,
    }
}

/// The node at `keys` below `value`, through documents only.
pub(super) fn dig<'value>(value: Option<&'value Bson>, keys: &[&str]) -> Option<&'value Bson> {
    keys.iter().try_fold(value?, |node, key| field(node, key))
}

/// The document a field holds, if it holds one.
pub(super) fn document(value: Option<&Bson>) -> Option<&Document> {
    match value {
        Some(Bson::Document(document)) => Some(document),
        _ => None,
    }
}

/// The identity `value` stores, in any of its BSON forms.
pub(super) fn id(value: Option<&Bson>) -> Option<String> {
    value.and_then(mxrs_bson::extract_id)
}

/// Whether a document's `$Type` is `kind`.
pub(super) fn has_type(document: &Document, kind: &str) -> bool {
    document.get_str("$Type").is_ok_and(|stored| stored == kind)
}

/// Whether `value` is the string `text`.
pub(super) fn is_str(value: Option<&Bson>, text: &str) -> bool {
    matches!(value, Some(Bson::String(stored)) if stored == text)
}

/// Whether a value counts as set: present, and neither null nor `false`.
pub(super) fn truthy(value: Option<&Bson>) -> bool {
    !matches!(value, None | Some(Bson::Null) | Some(Bson::Boolean(false)))
}

fn is_integer(value: &Bson) -> bool {
    matches!(value, Bson::Int32(_) | Bson::Int64(_))
}

/// An integer value of either BSON width.
pub(super) fn integer(value: Option<&Bson>) -> Option<i64> {
    match value {
        Some(Bson::Int32(number)) => Some(i64::from(*number)),
        Some(Bson::Int64(number)) => Some(*number),
        _ => None,
    }
}

/// An integer as BSON stores it: 32 bits when it fits.
pub(super) fn integer_bson(number: i64) -> Bson {
    i32::try_from(number).map_or(Bson::Int64(number), Bson::Int32)
}

/// Whether two documents hold the same values.
pub(super) fn same_document(left: &Document, right: &Document) -> bool {
    documents_equal(left, right, false)
}

/// Whether two nodes hold the same values.
pub(super) fn same(left: &Bson, right: &Bson) -> bool {
    equal(left, right, false)
}

/// Whether two nodes declare the same schema: the same values, identities
/// aside.
pub(super) fn same_schema(left: &Bson, right: &Bson) -> bool {
    equal(left, right, true)
}

fn equal(left: &Bson, right: &Bson, ignore_identity: bool) -> bool {
    match (left, right) {
        (Bson::Document(left), Bson::Document(right)) => {
            documents_equal(left, right, ignore_identity)
        }
        (Bson::Array(left), Bson::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| equal(left, right, ignore_identity))
        }
        _ => match (number(left), number(right)) {
            (Some(left), Some(right)) => left == right,
            _ => left == right,
        },
    }
}

fn documents_equal(left: &Document, right: &Document, ignore_identity: bool) -> bool {
    let kept = |key: &&String| !ignore_identity || !IDENTITY_KEYS.contains(&key.as_str());
    left.keys().filter(kept).count() == right.keys().filter(kept).count()
        && left
            .iter()
            .filter(|(key, _)| kept(key))
            .all(|(key, value)| {
                right
                    .get(key)
                    .is_some_and(|other| equal(value, other, ignore_identity))
            })
}

fn number(value: &Bson) -> Option<f64> {
    match value {
        // Mendix integers stay far inside the range a double holds exactly.
        Bson::Int32(number) => Some(f64::from(*number)),
        Bson::Int64(number) => Some(*number as f64),
        Bson::Double(number) => Some(*number),
        _ => None,
    }
}

/// A scalar as mxrb interpolates it into text: a string itself, null
/// empty, a number or boolean as written.
pub(super) fn to_text(value: Option<&Bson>) -> String {
    match value {
        Some(Bson::String(text)) => text.clone(),
        Some(Bson::Boolean(flag)) => flag.to_string(),
        Some(Bson::Int32(number)) => number.to_string(),
        Some(Bson::Int64(number)) => number.to_string(),
        Some(Bson::Double(number)) => float_text(*number),
        Some(Bson::Document(_) | Bson::Array(_)) => inspect(value),
        _ => String::new(),
    }
}

fn float_text(number: f64) -> String {
    if number.is_finite() && number.fract() == 0.0 && number.abs() < 1e16 {
        format!("{number:.1}")
    } else {
        number.to_string()
    }
}

/// A value as mxrb's messages quote it (Ruby's `inspect`).
pub(super) fn inspect(value: Option<&Bson>) -> String {
    match value {
        None | Some(Bson::Null) => "nil".to_string(),
        Some(Bson::String(text)) => inspect_str(text),
        Some(Bson::Array(items)) => format!(
            "[{}]",
            items
                .iter()
                .map(|item| inspect(Some(item)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Some(Bson::Document(document)) => format!(
            "{{{}}}",
            document
                .iter()
                .map(|(key, value)| format!("{} => {}", inspect_str(key), inspect(Some(value))))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Some(Bson::Binary(binary)) => format!("<BSON::Binary {} bytes>", binary.bytes.len()),
        Some(other) => to_text(Some(other)),
    }
}

/// A string as Ruby's `inspect` quotes it.
pub(super) fn inspect_str(text: &str) -> String {
    let mut quoted = String::from("\"");
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\t' => quoted.push_str("\\t"),
            '\r' => quoted.push_str("\\r"),
            '\u{c}' => quoted.push_str("\\f"),
            '\u{b}' => quoted.push_str("\\v"),
            '\u{8}' => quoted.push_str("\\b"),
            '\u{7}' => quoted.push_str("\\a"),
            '\u{1b}' => quoted.push_str("\\e"),
            '#' if matches!(characters.peek(), Some('{' | '$' | '@')) => quoted.push_str("\\#"),
            '\u{7f}' => quoted.push_str("\\x7F"),
            control if control.is_control() => {
                quoted.push_str(&format!("\\u{:04X}", u32::from(control)));
            }
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

/// The leading integer of a version string, as Ruby's `String#to_i` reads
/// it: `"11.12.1"` is 11, and text without one is 0.
pub(super) fn ruby_to_i(text: &str) -> i64 {
    let text = text.trim_start();
    let (sign, digits) = match text.as_bytes().first() {
        Some(b'-') => (-1, &text[1..]),
        Some(b'+') => (1, &text[1..]),
        _ => (1, text),
    };
    let mut value: i64 = 0;
    let mut previous_digit = false;
    let mut characters = digits.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '0'..='9' => {
                value = value
                    .saturating_mul(10)
                    .saturating_add(i64::from(character as u8 - b'0'));
                previous_digit = true;
            }
            // Ruby reads `1_000` as one thousand: an underscore between
            // digits continues the number.
            '_' if previous_digit && characters.peek().is_some_and(char::is_ascii_digit) => {
                previous_digit = false;
            }
            _ => break,
        }
    }
    sign * value
}
