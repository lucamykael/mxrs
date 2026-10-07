//! JSON structures: the shape of a JSON document a mapping reads or
//! writes. The model stores the snippet the structure was made from and the
//! tree of elements Studio Pro derives from it, each of which a developer
//! may since have changed — a name, a type, a length.
//!
//! [`JsonStructureDecl::derive`] derives that tree as Studio Pro does, so a
//! declaration states the snippet and only what differs from it.

use crate::{ExportLevel, NativeDocument, NativeValue};

/// What an element of a JSON structure is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonElementType {
    Object,
    Array,
    /// The element an array of values holds each value in.
    Wrapper,
    Value,
}

impl JsonElementType {
    pub fn as_str(self) -> &'static str {
        match self {
            JsonElementType::Object => "Object",
            JsonElementType::Array => "Array",
            JsonElementType::Wrapper => "Wrapper",
            JsonElementType::Value => "Value",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "Object" => Self::Object,
            "Array" => Self::Array,
            "Wrapper" => Self::Wrapper,
            "Value" => Self::Value,
            _ => return None,
        })
    }
}

/// What a value of a JSON structure holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonPrimitiveType {
    /// Nothing a value says: an object, an array, or `null`.
    Unknown,
    String,
    Integer,
    Long,
    Decimal,
    Boolean,
    DateTime,
}

impl JsonPrimitiveType {
    pub fn as_str(self) -> &'static str {
        match self {
            JsonPrimitiveType::Unknown => "Unknown",
            JsonPrimitiveType::String => "String",
            JsonPrimitiveType::Integer => "Integer",
            JsonPrimitiveType::Long => "Long",
            JsonPrimitiveType::Decimal => "Decimal",
            JsonPrimitiveType::Boolean => "Boolean",
            JsonPrimitiveType::DateTime => "DateTime",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "Unknown" => Self::Unknown,
            "String" => Self::String,
            "Integer" => Self::Integer,
            "Long" => Self::Long,
            "Decimal" => Self::Decimal,
            "Boolean" => Self::Boolean,
            "DateTime" => Self::DateTime,
            _ => return None,
        })
    }
}

/// One element of a JSON structure, and the elements it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonElement {
    /// Where the element is: `(Object)|order|lines|(Object)|number`.
    pub path: String,
    pub element_type: JsonElementType,
    pub primitive_type: JsonPrimitiveType,
    /// The name a mapping knows the element by.
    pub exposed_name: String,
    pub exposed_item_name: String,
    pub max_length: i32,
    pub min_occurs: i32,
    /// `-1`: as many as there are.
    pub max_occurs: i32,
    pub nillable: bool,
    pub is_default_type: bool,
    pub fraction_digits: i32,
    pub total_digits: i32,
    /// The value the snippet gives, as JSON writes it.
    pub original_value: String,
    pub error_message: String,
    pub warning_message: String,
    pub children: Vec<JsonElement>,
}

impl JsonElement {
    fn new(path: String, element_type: JsonElementType, exposed_name: String) -> Self {
        Self {
            path,
            element_type,
            primitive_type: JsonPrimitiveType::Unknown,
            exposed_name,
            exposed_item_name: String::new(),
            max_length: -1,
            min_occurs: 0,
            max_occurs: 1,
            nillable: true,
            is_default_type: false,
            fraction_digits: -1,
            total_digits: -1,
            original_value: String::new(),
            error_message: String::new(),
            warning_message: String::new(),
            children: Vec::new(),
        }
    }

    /// The element at `path`, this one or one it holds.
    pub fn find_mut(&mut self, path: &str) -> Option<&mut JsonElement> {
        if self.path == path {
            return Some(self);
        }
        if !path.starts_with(&self.path) {
            return None;
        }
        self.children
            .iter_mut()
            .find_map(|child| child.find_mut(path))
    }

    /// Every element, this one first, in the order the tree holds them.
    pub fn walk(&self) -> Vec<&JsonElement> {
        let mut all = vec![self];
        for child in &self.children {
            all.extend(child.walk());
        }
        all
    }

    fn from_document(document: &NativeDocument) -> Option<Self> {
        if document.ty != "JsonStructures$JsonElement" {
            return None;
        }
        let number = |key: &str| match document.get(key)? {
            NativeValue::Int32(value) => Some(*value),
            NativeValue::Int64(value) => i32::try_from(*value).ok(),
            _ => None,
        };
        let flag = |key: &str| match document.get(key)? {
            NativeValue::Bool(value) => Some(*value),
            _ => None,
        };
        let text = |key: &str| document.text(key).map(str::to_string);
        Some(Self {
            path: text("Path")?,
            element_type: JsonElementType::parse(document.text("ElementType")?)?,
            primitive_type: JsonPrimitiveType::parse(document.text("PrimitiveType")?)?,
            exposed_name: text("ExposedName")?,
            exposed_item_name: text("ExposedItemName")?,
            max_length: number("MaxLength")?,
            min_occurs: number("MinOccurs")?,
            max_occurs: number("MaxOccurs")?,
            nillable: flag("Nillable")?,
            is_default_type: flag("IsDefaultType")?,
            fraction_digits: number("FractionDigits")?,
            total_digits: number("TotalDigits")?,
            original_value: text("OriginalValue")?,
            error_message: text("ErrorMessage")?,
            warning_message: text("WarningMessage")?,
            children: elements(document.get("Children")?)?,
        })
    }

    fn document(&self) -> NativeDocument {
        NativeDocument::new("JsonStructures$JsonElement")
            .with("Children", element_list(&self.children))
            .with("ElementType", self.element_type.as_str())
            .with("ErrorMessage", self.error_message.as_str())
            .with("ExposedItemName", self.exposed_item_name.as_str())
            .with("ExposedName", self.exposed_name.as_str())
            .with("FractionDigits", NativeValue::Int32(self.fraction_digits))
            .with("IsDefaultType", self.is_default_type)
            .with("MaxLength", NativeValue::Int32(self.max_length))
            .with("MaxOccurs", NativeValue::Int32(self.max_occurs))
            .with("MinOccurs", NativeValue::Int32(self.min_occurs))
            .with("Nillable", self.nillable)
            .with("OriginalValue", self.original_value.as_str())
            .with("Path", self.path.as_str())
            .with("PrimitiveType", self.primitive_type.as_str())
            .with("TotalDigits", NativeValue::Int32(self.total_digits))
            .with("WarningMessage", self.warning_message.as_str())
    }
}

fn elements(value: &NativeValue) -> Option<Vec<JsonElement>> {
    match value {
        NativeValue::List(2, items) => items
            .iter()
            .map(|item| match item {
                NativeValue::Document(element) => JsonElement::from_document(element),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

fn element_list(elements: &[JsonElement]) -> NativeValue {
    NativeValue::List(
        2,
        elements
            .iter()
            .map(|element| NativeValue::Document(element.document()))
            .collect(),
    )
}

/// A JSON structure of a module: its snippet and the elements derived from
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonStructureDecl {
    pub name: String,
    pub documentation: String,
    pub snippet: String,
    pub elements: Vec<JsonElement>,
    pub excluded: bool,
    pub export_level: ExportLevel,
}

impl JsonStructureDecl {
    /// The structure Studio Pro makes of `snippet`, or why the snippet is
    /// not JSON.
    pub fn derive(name: impl Into<String>, snippet: impl Into<String>) -> Result<Self, String> {
        let snippet = snippet.into();
        let value = Parser::new(&snippet).document()?;
        let mut names = Names::default();
        let root = match &value {
            Json::Array(_) => "(Array)",
            _ => "(Object)",
        };
        let mut element = derive(&value, root.to_string(), "Root".to_string(), &mut names);
        element.min_occurs = 1;
        Ok(Self {
            name: name.into(),
            documentation: String::new(),
            snippet,
            elements: vec![element],
            excluded: false,
            export_level: ExportLevel::Hidden,
        })
    }

    /// The element at `path`.
    pub fn element_mut(&mut self, path: &str) -> Option<&mut JsonElement> {
        self.elements
            .iter_mut()
            .find_map(|element| element.find_mut(path))
    }

    /// The declaration a stored structure is, when [`Self::document`] states
    /// that document again.
    pub fn from_document(document: &NativeDocument) -> Option<Self> {
        let declaration = Self::read(document)?;
        declaration
            .document()
            .says_the_same(document)
            .then_some(declaration)
    }

    /// What a stored structure says, whatever order its fields are stored
    /// in: what a mapping of it maps.
    pub fn read(document: &NativeDocument) -> Option<Self> {
        if document.ty != "JsonStructures$JsonStructure" {
            return None;
        }
        Some(Self {
            name: document.text("Name")?.to_string(),
            documentation: document.text("Documentation")?.to_string(),
            snippet: document.text("JsonSnippet")?.to_string(),
            elements: elements(document.get("Elements")?)?,
            excluded: matches!(document.get("Excluded")?, NativeValue::Bool(true)),
            export_level: match document.text("ExportLevel")? {
                "Hidden" => ExportLevel::Hidden,
                "Published" => ExportLevel::Published,
                _ => return None,
            },
        })
    }

    /// The document the model stores for the structure, its fields in the
    /// order Studio Pro stores them.
    pub fn document(&self) -> NativeDocument {
        NativeDocument::new("JsonStructures$JsonStructure")
            .with("Documentation", self.documentation.as_str())
            .with("Elements", element_list(&self.elements))
            .with("Excluded", self.excluded)
            .with(
                "ExportLevel",
                match self.export_level {
                    ExportLevel::Hidden => "Hidden",
                    ExportLevel::Published => "Published",
                },
            )
            .with("JsonSnippet", self.snippet.as_str())
            .with("Name", self.name.as_str())
    }
}

/// The names given so far to what a mapping makes an entity of — objects,
/// arrays, wrappers — which Studio Pro keeps apart by a number.
#[derive(Default)]
struct Names(std::collections::HashSet<String>);

impl Names {
    fn unique(&mut self, name: String) -> String {
        let mut candidate = name.clone();
        let mut number = 1;
        while !self.0.insert(candidate.to_lowercase()) {
            number += 1;
            candidate = format!("{name}_{number}");
        }
        candidate
    }
}

fn derive(value: &Json, path: String, name: String, names: &mut Names) -> JsonElement {
    match value {
        Json::Object(members) => {
            let mut element = JsonElement::new(path, JsonElementType::Object, names.unique(name));
            for (key, member) in members {
                element.children.push(derive(
                    member,
                    format!("{}|{key}", element.path),
                    exposed_name(key),
                    names,
                ));
            }
            element
        }
        Json::Array(items) => {
            let mut element =
                JsonElement::new(path, JsonElementType::Array, names.unique(name.clone()));
            element.children = items_of(items, &element.path, &name, names);
            element
        }
        value => {
            let mut element = JsonElement::new(path, JsonElementType::Value, name);
            let (primitive_type, original_value) = primitive(value);
            element.primitive_type = primitive_type;
            element.original_value = original_value;
            if primitive_type == JsonPrimitiveType::String {
                element.max_length = 0;
            }
            element
        }
    }
}

/// What an array named `name` holds: one object with every member its
/// objects have, another array, or a wrapper around its values.
fn items_of(items: &[Json], path: &str, name: &str, names: &mut Names) -> Vec<JsonElement> {
    let Some(first) = items.iter().find(|item| !matches!(item, Json::Null)) else {
        return Vec::new();
    };
    let item = match first {
        Json::Object(_) => {
            let mut members: Vec<(String, Json)> = Vec::new();
            for item in items {
                let Json::Object(item) = item else { continue };
                for (key, value) in item {
                    match members.iter_mut().find(|(known, _)| known == key) {
                        Some((_, known)) if matches!(known, Json::Null) => *known = value.clone(),
                        Some(_) => {}
                        None => members.push((key.clone(), value.clone())),
                    }
                }
            }
            derive(
                &Json::Object(members),
                format!("{path}|(Object)"),
                singular(name),
                names,
            )
        }
        Json::Array(nested) => {
            let item_name = singular(name);
            let mut element = JsonElement::new(
                format!("{path}|(Array)"),
                JsonElementType::Array,
                names.unique(item_name.clone()),
            );
            element.children = items_of(nested, &element.path, &item_name, names);
            element
        }
        value => {
            let mut wrapper = JsonElement::new(
                format!("{path}|(Wrapper)"),
                JsonElementType::Wrapper,
                names.unique("Wrapper".to_string()),
            );
            wrapper.children.push(derive(
                value,
                format!("{}|(Value)", wrapper.path),
                "Value".to_string(),
                names,
            ));
            wrapper
        }
    };
    let mut item = item;
    item.max_occurs = -1;
    vec![item]
}

/// What Studio Pro names the element of a member called `key`: an
/// identifier, its first letter capital, and a name Mendix keeps for itself
/// marked as the member's.
fn exposed_name(key: &str) -> String {
    const RESERVED: &[&str] = &[
        "id",
        "type",
        "guid",
        "changeddate",
        "createddate",
        "owner",
        "changedby",
        "submetaobjectname",
        "context",
    ];
    if RESERVED.contains(&key.to_lowercase().as_str()) {
        return format!("_{key}");
    }
    let mut name: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if let Some(first) = name.chars().next()
        && first.is_ascii_lowercase()
    {
        name.replace_range(..1, &first.to_ascii_uppercase().to_string());
    }
    name
}

/// What one of the things an array called `name` holds is called.
fn singular(name: &str) -> String {
    if let Some(stem) = name.strip_suffix("ies") {
        return format!("{stem}y");
    }
    match name.strip_suffix('s') {
        Some(stem) if !stem.ends_with('s') && !stem.is_empty() => stem.to_string(),
        _ => format!("{name}Item"),
    }
}

/// The type a value holds and the value as Studio Pro keeps it.
fn primitive(value: &Json) -> (JsonPrimitiveType, String) {
    match value {
        Json::Null => (JsonPrimitiveType::Unknown, "null".to_string()),
        Json::Bool(value) => (JsonPrimitiveType::Boolean, value.to_string()),
        Json::Number(text) => {
            let integral = !text.contains(['.', 'e', 'E']);
            let ty = match text.parse::<i64>() {
                Ok(number) if integral && i32::try_from(number).is_ok() => {
                    JsonPrimitiveType::Integer
                }
                Ok(_) if integral => JsonPrimitiveType::Long,
                _ => JsonPrimitiveType::Decimal,
            };
            (ty, text.clone())
        }
        Json::String(text) => match datetime(text) {
            Some(normalized) => (JsonPrimitiveType::DateTime, quoted(&normalized)),
            None => (JsonPrimitiveType::String, quoted(text)),
        },
        Json::Object(_) | Json::Array(_) => (JsonPrimitiveType::Unknown, String::new()),
    }
}

/// `text` as a JSON string, escaped as Studio Pro's serializer escapes one:
/// quotes, backslashes and control characters, nothing else.
fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\u{85}' | '\u{2028}' | '\u{2029}' => {
                out.push_str(&format!("\\u{:04x}", u32::from(c)));
            }
            c if u32::from(c) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A UTC date and time as Studio Pro keeps it — seven digits of a second —
/// when `text` is one.
fn datetime(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let digits = |range: std::ops::Range<usize>| {
        bytes
            .get(range)
            .is_some_and(|part| part.iter().all(u8::is_ascii_digit))
    };
    let shaped = text.len() >= 20
        && digits(0..4)
        && bytes[4] == b'-'
        && digits(5..7)
        && bytes[7] == b'-'
        && digits(8..10)
        && bytes[10] == b'T'
        && digits(11..13)
        && bytes[13] == b':'
        && digits(14..16)
        && bytes[16] == b':'
        && digits(17..19);
    let rest = text.get(19..)?.strip_suffix('Z')?;
    if !shaped {
        return None;
    }
    let fraction = match rest.strip_prefix('.') {
        Some(fraction) if !fraction.is_empty() && fraction.bytes().all(|b| b.is_ascii_digit()) => {
            fraction
        }
        None if rest.is_empty() => "",
        _ => return None,
    };
    let fraction: String = fraction
        .chars()
        .chain(std::iter::repeat('0'))
        .take(7)
        .collect();
    Some(format!("{}.{fraction}Z", &text[..19]))
}

/// A JSON value, its object members in the order written and its numbers
/// as written.
#[derive(Debug, Clone)]
enum Json {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

struct Parser<'a> {
    text: &'a str,
    at: usize,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str) -> Self {
        Self { text, at: 0 }
    }

    fn document(mut self) -> Result<Json, String> {
        let value = self.value()?;
        self.space();
        if self.at != self.text.len() {
            return Err(self.error("text after the JSON value"));
        }
        Ok(value)
    }

    fn error(&self, what: &str) -> String {
        let line = self.text[..self.at].matches('\n').count() + 1;
        format!("the snippet is not JSON: {what} on line {line}")
    }

    fn space(&mut self) {
        let rest = &self.text[self.at..];
        self.at += rest.len() - rest.trim_start().len();
    }

    fn peek(&self) -> Option<char> {
        self.text[self.at..].chars().next()
    }

    fn eat(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.at += expected.len_utf8();
            true
        } else {
            false
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.space();
        match self.peek() {
            Some('{') => self.object(),
            Some('[') => self.array(),
            Some('"') => self.string().map(Json::String),
            Some('t') => self.word("true", Json::Bool(true)),
            Some('f') => self.word("false", Json::Bool(false)),
            Some('n') => self.word("null", Json::Null),
            Some(c) if c == '-' || c.is_ascii_digit() => self.number(),
            _ => Err(self.error("a value is expected")),
        }
    }

    fn word(&mut self, word: &str, value: Json) -> Result<Json, String> {
        if self.text[self.at..].starts_with(word) {
            self.at += word.len();
            Ok(value)
        } else {
            Err(self.error("a value is expected"))
        }
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.at;
        let rest = &self.text[start..];
        let length = rest
            .find(|c: char| !(c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E')))
            .unwrap_or(rest.len());
        let text = &rest[..length];
        if text.parse::<f64>().is_err() {
            return Err(self.error("a number is malformed"));
        }
        self.at += length;
        Ok(Json::Number(text.to_string()))
    }

    fn string(&mut self) -> Result<String, String> {
        self.at += 1;
        let mut out = String::new();
        loop {
            let Some(c) = self.peek() else {
                return Err(self.error("a string is not closed"));
            };
            self.at += c.len_utf8();
            match c {
                '"' => return Ok(out),
                '\\' => {
                    let Some(escaped) = self.peek() else {
                        return Err(self.error("a string is not closed"));
                    };
                    self.at += escaped.len_utf8();
                    out.push(match escaped {
                        '"' | '\\' | '/' => escaped,
                        'b' => '\u{8}',
                        'f' => '\u{c}',
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        'u' => self.unicode()?,
                        _ => return Err(self.error("a string has an unknown escape")),
                    });
                }
                c if u32::from(c) < 0x20 => {
                    return Err(self.error("a string holds a control character"));
                }
                c => out.push(c),
            }
        }
    }

    /// The character a `\u` escape names, a surrogate pair's whole.
    fn unicode(&mut self) -> Result<char, String> {
        let unit = self.hex()?;
        let code = if (0xD800..0xDC00).contains(&unit) {
            if !self.text[self.at..].starts_with("\\u") {
                return Err(self.error("a surrogate pair is broken"));
            }
            self.at += 2;
            let low = self.hex()?;
            if !(0xDC00..0xE000).contains(&low) {
                return Err(self.error("a surrogate pair is broken"));
            }
            0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00)
        } else {
            unit
        };
        char::from_u32(code).ok_or_else(|| self.error("a string names no character"))
    }

    fn hex(&mut self) -> Result<u32, String> {
        let digits = self
            .text
            .get(self.at..self.at + 4)
            .filter(|digits| digits.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| self.error("a `\\u` escape is malformed"))?;
        self.at += 4;
        Ok(u32::from_str_radix(digits, 16).expect("four hex digits"))
    }

    fn array(&mut self) -> Result<Json, String> {
        self.at += 1;
        let mut items = Vec::new();
        self.space();
        if self.eat(']') {
            return Ok(Json::Array(items));
        }
        loop {
            items.push(self.value()?);
            self.space();
            if self.eat(']') {
                return Ok(Json::Array(items));
            }
            if !self.eat(',') {
                return Err(self.error("`,` or `]` is expected"));
            }
        }
    }

    fn object(&mut self) -> Result<Json, String> {
        self.at += 1;
        let mut members = Vec::new();
        self.space();
        if self.eat('}') {
            return Ok(Json::Object(members));
        }
        loop {
            self.space();
            if self.peek() != Some('"') {
                return Err(self.error("a member's name is expected"));
            }
            let key = self.string()?;
            self.space();
            if !self.eat(':') {
                return Err(self.error("`:` is expected"));
            }
            let value = self.value()?;
            members.push((key, value));
            self.space();
            if self.eat('}') {
                return Ok(Json::Object(members));
            }
            if !self.eat(',') {
                return Err(self.error("`,` or `}` is expected"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(element: &JsonElement) -> Vec<String> {
        element
            .walk()
            .into_iter()
            .map(|element| {
                format!(
                    "{} {} {} {}..{} {}",
                    element.path,
                    element.element_type.as_str(),
                    element.exposed_name,
                    element.min_occurs,
                    element.max_occurs,
                    element.original_value
                )
            })
            .collect()
    }

    /// A snippet's members are its elements, named as a mapping knows them;
    /// an array holds one object of every member its objects have, and an
    /// array of values a wrapper around one.
    #[test]
    fn a_snippet_derives_the_elements_studio_pro_makes_of_it() {
        let structure = JsonStructureDecl::derive(
            "JSON_Order",
            r#"{"id": 7, "order date": "2025-09-23T10:15:30Z",
                "lines": [{"sku": "A", "price": null}, {"price": 1.5, "qty": 3000000000}],
                "tags": ["new", "paid"], "rows": [[1]], "note": null, "paid": true}"#,
        )
        .unwrap();
        assert_eq!(
            tree(&structure.elements[0]),
            [
                "(Object) Object Root 1..1 ",
                "(Object)|id Value _id 0..1 7",
                "(Object)|order date Value Order_date 0..1 \"2025-09-23T10:15:30.0000000Z\"",
                "(Object)|lines Array Lines 0..1 ",
                "(Object)|lines|(Object) Object Line 0..-1 ",
                "(Object)|lines|(Object)|sku Value Sku 0..1 \"A\"",
                "(Object)|lines|(Object)|price Value Price 0..1 1.5",
                "(Object)|lines|(Object)|qty Value Qty 0..1 3000000000",
                "(Object)|tags Array Tags 0..1 ",
                "(Object)|tags|(Wrapper) Wrapper Wrapper 0..-1 ",
                "(Object)|tags|(Wrapper)|(Value) Value Value 0..1 \"new\"",
                "(Object)|rows Array Rows 0..1 ",
                "(Object)|rows|(Array) Array Row 0..-1 ",
                "(Object)|rows|(Array)|(Wrapper) Wrapper Wrapper_2 0..-1 ",
                "(Object)|rows|(Array)|(Wrapper)|(Value) Value Value 0..1 1",
                "(Object)|note Value Note 0..1 null",
                "(Object)|paid Value Paid 0..1 true",
            ]
        );
        let all = structure.elements[0].walk();
        let kind = |path: &str| {
            all.iter()
                .find(|element| element.path == path)
                .unwrap()
                .primitive_type
        };
        assert_eq!(kind("(Object)|order date"), JsonPrimitiveType::DateTime);
        assert_eq!(
            kind("(Object)|lines|(Object)|price"),
            JsonPrimitiveType::Decimal
        );
        assert_eq!(kind("(Object)|lines|(Object)|qty"), JsonPrimitiveType::Long);
        assert_eq!(kind("(Object)|id"), JsonPrimitiveType::Integer);
        assert_eq!(kind("(Object)|note"), JsonPrimitiveType::Unknown);
        // The document reads back as the declaration that states it.
        let document = structure.document();
        assert_eq!(JsonStructureDecl::from_document(&document), Some(structure));
    }

    #[test]
    fn a_snippet_that_is_not_json_says_where() {
        let error = JsonStructureDecl::derive("JSON_Bad", "{\n  \"a\": }").unwrap_err();
        assert_eq!(
            error,
            "the snippet is not JSON: a value is expected on line 2"
        );
    }
}
