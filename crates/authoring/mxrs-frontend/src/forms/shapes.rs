//! The elements a project's documents are made of, read off the documents
//! themselves: each type's fields in stored order and, for each field, the
//! value most of the project's documents give it.

use std::collections::{BTreeMap, HashSet};

use mxrs_ir::{NativeDocument, NativeValue};

use super::{CUSTOM_WIDGET, Field, FieldDefault, Shape, Shapes, Vocabulary, WidgetDefinition};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Scalar {
    Null,
    Bool(bool),
    Int32(i32),
    Int64(i64),
    Text(String),
}

impl Scalar {
    fn value(&self) -> NativeValue {
        match self {
            Self::Null => NativeValue::Null,
            Self::Bool(value) => NativeValue::Bool(*value),
            Self::Int32(value) => NativeValue::Int32(*value),
            Self::Int64(value) => NativeValue::Int64(*value),
            Self::Text(value) => NativeValue::Text(value.clone()),
        }
    }

    /// The value of this kind that says nothing.
    fn zero(&self) -> NativeValue {
        match self {
            Self::Null => NativeValue::Null,
            Self::Bool(_) => NativeValue::Bool(false),
            Self::Int32(_) => NativeValue::Int32(0),
            Self::Int64(_) => NativeValue::Int64(0),
            Self::Text(_) => NativeValue::Text(String::new()),
        }
    }

    fn kind(&self) -> u8 {
        match self {
            Self::Text(_) => 0,
            Self::Int32(_) => 1,
            Self::Int64(_) => 2,
            Self::Bool(_) => 3,
            Self::Null => 4,
        }
    }
}

/// A type as it is stored with one set of fields.
type Variant = (String, Vec<String>);

fn variant_of(document: &NativeDocument) -> Variant {
    (
        document.ty.clone(),
        document.fields.iter().map(|(key, _)| key.clone()).collect(),
    )
}

#[derive(Default)]
struct FieldTally {
    total: usize,
    scalars: BTreeMap<Scalar, usize>,
    documents: BTreeMap<Variant, usize>,
    lists: BTreeMap<i32, usize>,
    pointers: usize,
    /// Whether some list here holds a document, and whether one holds
    /// something else.
    holds_documents: bool,
    holds_others: bool,
}

#[derive(Default)]
struct Tally {
    count: usize,
    fields: BTreeMap<String, FieldTally>,
}

/// The most frequent entry: the highest count, and the smallest key among
/// those that share it.
fn most<K: Ord + Clone>(counts: &BTreeMap<K, usize>) -> Option<(K, usize)> {
    counts
        .iter()
        .max_by(|(left_key, left), (right_key, right)| {
            left.cmp(right).then_with(|| right_key.cmp(left_key))
        })
        .map(|(key, count)| (key.clone(), *count))
}

fn tally(document: &NativeDocument, tallies: &mut BTreeMap<Variant, Tally>) {
    let entry = tallies.entry(variant_of(document)).or_default();
    entry.count += 1;
    for (key, value) in &document.fields {
        let field = entry.fields.entry(key.clone()).or_default();
        field.total += 1;
        match value {
            NativeValue::Null => *field.scalars.entry(Scalar::Null).or_default() += 1,
            NativeValue::Bool(value) => {
                *field.scalars.entry(Scalar::Bool(*value)).or_default() += 1;
            }
            NativeValue::Int32(value) => {
                *field.scalars.entry(Scalar::Int32(*value)).or_default() += 1;
            }
            NativeValue::Int64(value) => {
                *field.scalars.entry(Scalar::Int64(*value)).or_default() += 1;
            }
            NativeValue::Text(value) => {
                *field
                    .scalars
                    .entry(Scalar::Text(value.clone()))
                    .or_default() += 1;
            }
            NativeValue::Document(nested) => {
                *field.documents.entry(variant_of(nested)).or_default() += 1;
            }
            NativeValue::List(marker, items) => {
                *field.lists.entry(*marker).or_default() += 1;
                for item in items {
                    if matches!(item, NativeValue::Document(_)) {
                        field.holds_documents = true;
                    } else {
                        field.holds_others = true;
                    }
                }
            }
            NativeValue::Pointer(_) | NativeValue::Identity(_) => field.pointers += 1,
        }
    }
    for (_, value) in &document.fields {
        match value {
            NativeValue::Document(nested) => tally(nested, tallies),
            NativeValue::List(_, items) => {
                for item in items {
                    if let NativeValue::Document(nested) = item {
                        tally(nested, tallies);
                    }
                }
            }
            _ => {}
        }
    }
}

/// What a field holds by default; an element is named by its variant.
enum Mined {
    Value(NativeValue),
    Element(Variant),
    List(i32),
}

/// How many documents must hold a field before what most of them say is
/// what all say by default. With fewer, a default would be one document's
/// own content, moved out of the page that states it.
const ENOUGH: usize = 3;

fn default_of(key: &str, field: &FieldTally) -> Mined {
    let listed: usize = field.lists.values().sum();
    if listed == field.total
        && let Some((marker, _)) = most(&field.lists)
    {
        return Mined::List(marker);
    }
    let scalar = most(&field.scalars);
    let document = most(&field.documents);
    // A name is its element's own, however many share it.
    let shared = field.total >= ENOUGH && key != "Name";
    if let Some((value, count)) = &scalar
        && shared
        && count * 2 > field.total
    {
        return Mined::Value(value.value());
    }
    // The element a field mostly holds is its structure, not its content:
    // that element's own fields have their own defaults.
    if let Some((variant, count)) = &document
        && count * 2 > field.total
    {
        return Mined::Element(variant.clone());
    }
    // No value is what most documents say: the default says nothing, in
    // the kind of value the field mostly holds.
    let mut kinds: BTreeMap<u8, (usize, Scalar)> = BTreeMap::new();
    for (value, count) in &field.scalars {
        let entry = kinds
            .entry(value.kind())
            .or_insert_with(|| (0, value.clone()));
        entry.0 += count;
    }
    let scalars = kinds
        .into_iter()
        .max_by(|(left_kind, (left, _)), (right_kind, (right, _))| {
            left.cmp(right).then_with(|| right_kind.cmp(left_kind))
        })
        .map(|(_, (count, value))| (count, value));
    let documents: usize = field.documents.values().sum();
    match scalars {
        Some((count, value)) if count >= documents.max(field.pointers).max(listed) => {
            Mined::Value(value.zero())
        }
        _ => Mined::Value(NativeValue::Null),
    }
}

fn is_component(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The component each variant is written as: its type's name without its
/// namespace — with it where two namespaces share the name — and a number
/// after the first for a type stored with several sets of fields, the most
/// frequent first.
fn components(tallies: &BTreeMap<Variant, Tally>) -> BTreeMap<Variant, String> {
    let short = |ty: &str| ty.rsplit('$').next().unwrap_or(ty).to_string();
    let types: std::collections::BTreeSet<&String> = tallies.keys().map(|(ty, _)| ty).collect();
    let mut uses: BTreeMap<String, usize> = BTreeMap::new();
    for ty in &types {
        *uses.entry(short(ty)).or_default() += 1;
    }
    let mut named = BTreeMap::new();
    for ty in types {
        let name = short(ty);
        let base = if uses[&name] > 1 {
            ty.replace('$', "")
        } else {
            name
        };
        let mut variants: Vec<(&Variant, usize)> = tallies
            .iter()
            .filter(|((other, _), _)| other == ty)
            .map(|(variant, tally)| (variant, tally.count))
            .collect();
        variants.sort_by(|(left_keys, left), (right_keys, right)| {
            right.cmp(left).then_with(|| left_keys.cmp(right_keys))
        });
        for (index, (variant, _)) in variants.into_iter().enumerate() {
            let component = if index == 0 {
                base.clone()
            } else {
                format!("{base}_{}", index + 1)
            };
            named.insert(variant.clone(), component);
        }
    }
    named
}

/// The elements and pluggable widgets `documents` are made of.
pub fn mine(documents: &[&NativeDocument]) -> Vocabulary {
    let mut tallies = BTreeMap::new();
    for document in documents {
        tally(document, &mut tallies);
    }
    let names = components(&tallies);
    let mut shapes: BTreeMap<String, Shape> = BTreeMap::new();
    for (variant, counted) in &tallies {
        let (ty, keys) = variant;
        let component = &names[variant];
        let props: Option<Vec<String>> = keys.iter().map(|key| super::prop_of(key)).collect();
        let Some(props) = props else {
            continue;
        };
        let distinct: HashSet<&String> = props.iter().collect();
        if !is_component(component) || distinct.len() != props.len() {
            continue;
        }
        let fields: Vec<Field> = keys
            .iter()
            .zip(props)
            .map(|(key, prop)| Field {
                key: key.clone(),
                prop,
                default: match default_of(key, &counted.fields[key]) {
                    Mined::Value(value) => FieldDefault::Value(value),
                    Mined::Element(nested) => FieldDefault::Element(names[&nested].clone()),
                    Mined::List(marker) => FieldDefault::List(marker),
                },
            })
            .collect();
        let lists: Vec<&String> = keys
            .iter()
            .filter(|key| {
                let field = &counted.fields[*key];
                field.holds_documents
                    && !field.holds_others
                    && field.lists.values().sum::<usize>() == field.total
            })
            .collect();
        let children = match lists.as_slice() {
            _ if lists.iter().any(|key| key.as_str() == "Widgets") => Some("Widgets".to_string()),
            [only] => Some((*only).clone()),
            _ => None,
        };
        shapes.insert(
            component.clone(),
            Shape {
                ty: ty.clone(),
                component: component.clone(),
                fields,
                children,
            },
        );
    }
    acyclic(&mut shapes);
    let shapes = Shapes::new(shapes.into_values().collect())
        .expect("mined shapes name each other and hold no cycle");

    let mut widgets: Vec<WidgetDefinition> = Vec::new();
    let mut uses: BTreeMap<String, usize> = BTreeMap::new();
    for document in documents {
        super::walk(document, "", &mut |_, nested| {
            if nested.ty != CUSTOM_WIDGET {
                return;
            }
            let Some(NativeValue::Document(ty)) = nested.get("Type") else {
                return;
            };
            if widgets.iter().any(|known| &known.ty == ty) {
                return;
            }
            let base = widget_name(ty.text("WidgetId").unwrap_or_default());
            let base = if shapes.component(&base).is_some() {
                format!("{base}Widget")
            } else {
                base
            };
            let count = uses.entry(base.clone()).or_default();
            *count += 1;
            let name = if *count == 1 {
                base
            } else {
                format!("{base}_{count}")
            };
            widgets.push(WidgetDefinition {
                name,
                ty: ty.clone(),
            });
        });
    }
    Vocabulary { shapes, widgets }
}

/// The component a pluggable widget is used by: the last word of its id,
/// as a type's name is written.
fn widget_name(id: &str) -> String {
    let word: String = id
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let mut letters = word.chars();
    match letters.next() {
        Some(first) if first.is_ascii_alphabetic() => {
            format!("{}{}", first.to_ascii_uppercase(), letters.as_str())
        }
        _ => format!("Widget{word}"),
    }
}

/// Drops each default that would make an element hold itself, and each
/// that names a type with no shape.
fn acyclic(shapes: &mut BTreeMap<String, Shape>) {
    fn visit(
        ty: &str,
        shapes: &mut BTreeMap<String, Shape>,
        open: &mut Vec<String>,
        done: &mut HashSet<String>,
    ) {
        if done.contains(ty) {
            return;
        }
        open.push(ty.to_string());
        let count = shapes[ty].fields.len();
        for index in 0..count {
            let FieldDefault::Element(nested) = shapes[ty].fields[index].default.clone() else {
                continue;
            };
            if !shapes.contains_key(&nested) || open.contains(&nested) {
                shapes.get_mut(ty).expect("a known type").fields[index].default =
                    FieldDefault::Value(NativeValue::Null);
                continue;
            }
            visit(&nested, shapes, open, done);
        }
        open.pop();
        done.insert(ty.to_string());
    }
    let types: Vec<String> = shapes.keys().cloned().collect();
    let mut done = HashSet::new();
    for ty in types {
        visit(&ty, shapes, &mut Vec::new(), &mut done);
    }
}

/// `src/mxrs/elements.ts`: each element, after the elements its defaults
/// hold.
pub fn render_elements(shapes: &Shapes) -> String {
    let mut out = String::from(
        "// The elements this project's pages, layouts and snippets are written with.\n\
         // Each is a kind of document the model stores, with the value every field\n\
         // has when a page says nothing about it: a page states only what differs.\n\
         // mxrs reads this file into every build, so a default changed here changes\n\
         // every page that leaves the field unsaid.\n\
         import { children, element, list, long } from \"@/mxrs/forms\";\n",
    );
    let mut ordered: Vec<&Shape> = shapes.iter().collect();
    ordered.sort_by(|left, right| left.component.cmp(&right.component));
    let mut written: HashSet<&str> = HashSet::new();
    fn write<'a>(
        shape: &'a Shape,
        shapes: &'a Shapes,
        written: &mut HashSet<&'a str>,
        out: &mut String,
    ) {
        if !written.insert(shape.component.as_str()) {
            return;
        }
        for field in &shape.fields {
            if let FieldDefault::Element(nested) = &field.default
                && let Some(nested) = shapes.component(nested)
            {
                write(nested, shapes, written, out);
            }
        }
        if shape.fields.is_empty() {
            out.push_str(&format!(
                "\nexport const {} = element({}, {{}});\n",
                shape.component,
                super::tsx::json(&shape.ty)
            ));
            return;
        }
        out.push_str(&format!(
            "\nexport const {} = element({}, {{\n",
            shape.component,
            super::tsx::json(&shape.ty)
        ));
        for field in &shape.fields {
            let value = match &field.default {
                FieldDefault::Value(NativeValue::Int64(value)) => format!("long({value})"),
                FieldDefault::Value(value) => {
                    super::tsx::scalar(value).expect("a default is a scalar")
                }
                FieldDefault::Element(nested) => nested.clone(),
                FieldDefault::List(marker) if shape.children.as_deref() == Some(&field.key) => {
                    format!("children({marker})")
                }
                FieldDefault::List(marker) => format!("list({marker})"),
            };
            out.push_str(&format!("  {}: {value},\n", field.prop));
        }
        out.push_str("});\n");
    }
    for shape in ordered {
        write(shape, shapes, &mut written, &mut out);
    }
    out
}
