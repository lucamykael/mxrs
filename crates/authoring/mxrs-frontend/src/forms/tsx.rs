//! TSX as it is written: the values a declaration is made of, laid out the
//! way a formatter would leave them.

use mxrs_ir::NativeValue;

/// The widest a line is written.
const WIDTH: usize = 100;

/// A value in a declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Js {
    /// An expression written as it stands: a literal or a call.
    Raw(String),
    Element(Element),
    Array(Vec<Js>),
    Object(Vec<(String, Js)>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Element {
    pub(crate) tag: String,
    pub(crate) attrs: Vec<(String, Attr)>,
    pub(crate) children: Vec<Element>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Attr {
    /// `name="text"`.
    Text(String),
    /// `name={value}`.
    Value(Js),
}

pub(crate) fn json(text: &str) -> String {
    serde_json::Value::String(text.to_string()).to_string()
}

/// `null`, a boolean, a 32-bit number or a text, as an expression.
pub(crate) fn scalar(value: &NativeValue) -> Option<String> {
    Some(match value {
        NativeValue::Null => "null".to_string(),
        NativeValue::Bool(value) => value.to_string(),
        NativeValue::Int32(value) => value.to_string(),
        NativeValue::Text(value) => text(value),
        _ => return None,
    })
}

/// Text as an expression: over several lines as it reads, in a template
/// literal, when it has lines a template keeps; a string literal otherwise.
pub(crate) fn text(text: &str) -> String {
    if !text.contains('\n') || text.contains('\r') {
        return json(text);
    }
    let escaped = text
        .replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace("${", "\\${");
    format!("`{escaped}`")
}

/// Text as a prop: between quotes when nothing in it would be read as
/// anything else there, as an expression otherwise.
pub(crate) fn text_attr(value: &str) -> Attr {
    let plain = value
        .chars()
        .all(|c| !matches!(c, '"' | '&' | '\\' | '\u{2028}' | '\u{2029}') && !c.is_control());
    if plain {
        Attr::Text(value.to_string())
    } else {
        Attr::Value(Js::Raw(text(value)))
    }
}

/// A key of an object literal: bare when it is an identifier.
pub(crate) fn key(name: &str) -> String {
    let identifier = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
    if identifier {
        name.to_string()
    } else {
        json(name)
    }
}

fn pad(indent: usize) -> String {
    " ".repeat(indent)
}

fn inline_attr(name: &str, attr: &Attr) -> Option<String> {
    Some(match attr {
        Attr::Text(text) => format!("{name}=\"{text}\""),
        Attr::Value(value) => format!("{name}={{{}}}", inline(value)?),
    })
}

/// The value on one line, when it has one line.
fn inline(value: &Js) -> Option<String> {
    match value {
        Js::Raw(text) => (!text.contains('\n')).then(|| text.clone()),
        Js::Element(element) => {
            if !element.children.is_empty() {
                return None;
            }
            let mut out = format!("<{}", element.tag);
            for (name, attr) in &element.attrs {
                out.push(' ');
                out.push_str(&inline_attr(name, attr)?);
            }
            out.push_str(" />");
            Some(out)
        }
        Js::Array(items) => {
            let items: Option<Vec<String>> = items.iter().map(inline).collect();
            Some(format!("[{}]", items?.join(", ")))
        }
        Js::Object(entries) if entries.is_empty() => Some("{}".to_string()),
        Js::Object(entries) => {
            let entries: Option<Vec<String>> = entries
                .iter()
                .map(|(name, value)| Some(format!("{}: {}", key(name), inline(value)?)))
                .collect();
            Some(format!("{{ {} }}", entries?.join(", ")))
        }
    }
}

/// The lines of `value`, the first of them indented by `indent` and so
/// much of the rest as is not text of its own.
pub(crate) fn lines(value: &Js, indent: usize) -> Vec<String> {
    lines_within(value, indent, 0)
}

/// [`lines`], where `taken` columns of the value's one line are someone
/// else's: it stays on one line only when it fits beside them.
fn lines_within(value: &Js, indent: usize, taken: usize) -> Vec<String> {
    if let Some(line) = inline(value)
        && indent + taken + line.chars().count() <= WIDTH
    {
        return vec![format!("{}{line}", pad(indent))];
    }
    match value {
        Js::Raw(text) => {
            let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
            lines[0] = format!("{}{}", pad(indent), lines[0]);
            lines
        }
        Js::Element(element) => element_lines(element, indent, taken),
        Js::Array(items) => {
            let mut out = vec![format!("{}[", pad(indent))];
            for item in items {
                out.extend(with_comma(lines(item, indent + 2)));
            }
            out.push(format!("{}]", pad(indent)));
            out
        }
        Js::Object(entries) => {
            let mut out = vec![format!("{}{{", pad(indent))];
            for (name, value) in entries {
                out.extend(entry_lines(name, value, indent + 2));
            }
            out.push(format!("{}}}", pad(indent)));
            out
        }
    }
}

fn with_comma(mut lines: Vec<String>) -> Vec<String> {
    lines.last_mut().expect("a value has a line").push(',');
    lines
}

fn entry_lines(name: &str, value: &Js, indent: usize) -> Vec<String> {
    // The key and the comma share the value's first and last lines.
    let key = key(name);
    let mut out = lines_within(value, indent, key.chars().count() + 3);
    out[0] = format!("{}{key}: {}", pad(indent), &out[0][indent..]);
    with_comma(out)
}

fn attr_lines(name: &str, attr: &Attr, indent: usize) -> Vec<String> {
    if let Some(line) = inline_attr(name, attr)
        && indent + line.chars().count() <= WIDTH
    {
        return vec![format!("{}{line}", pad(indent))];
    }
    let Attr::Value(value) = attr else {
        // Text between quotes has one line, however long.
        return vec![format!(
            "{}{}",
            pad(indent),
            inline_attr(name, attr).expect("text is one line")
        )];
    };
    match value {
        Js::Raw(text) => {
            // A text of several lines is its own lines: nothing is indented.
            let mut out: Vec<String> = text.split('\n').map(str::to_string).collect();
            out[0] = format!("{}{name}={{{}", pad(indent), out[0]);
            out.last_mut().expect("a text has a line").push('}');
            out
        }
        Js::Element(_) => {
            let mut out = vec![format!("{}{name}={{", pad(indent))];
            out.extend(lines(value, indent + 2));
            out.push(format!("{}}}", pad(indent)));
            out
        }
        Js::Array(items) => {
            let mut out = vec![format!("{}{name}={{[", pad(indent))];
            for item in items {
                out.extend(with_comma(lines(item, indent + 2)));
            }
            out.push(format!("{}]}}", pad(indent)));
            out
        }
        Js::Object(entries) => {
            let mut out = vec![format!("{}{name}={{{{", pad(indent))];
            for (name, value) in entries {
                out.extend(entry_lines(name, value, indent + 2));
            }
            out.push(format!("{}}}}}", pad(indent)));
            out
        }
    }
}

fn element_lines(element: &Element, indent: usize, taken: usize) -> Vec<String> {
    let attrs: Option<Vec<String>> = element
        .attrs
        .iter()
        .map(|(name, attr)| inline_attr(name, attr))
        .collect();
    let head = attrs.map(|attrs| {
        let mut head = format!("<{}", element.tag);
        for attr in attrs {
            head.push(' ');
            head.push_str(&attr);
        }
        head
    });
    let close = if element.children.is_empty() {
        "/>"
    } else {
        ">"
    };
    let mut out = Vec::new();
    match head {
        Some(head) if indent + taken + head.chars().count() + close.len() < WIDTH => {
            let space = if element.children.is_empty() { " " } else { "" };
            out.push(format!("{}{head}{space}{close}", pad(indent)));
        }
        _ => {
            out.push(format!("{}<{}", pad(indent), element.tag));
            for (name, attr) in &element.attrs {
                out.extend(attr_lines(name, attr, indent + 2));
            }
            out.push(format!("{}{close}", pad(indent)));
        }
    }
    if !element.children.is_empty() {
        for child in &element.children {
            out.extend(lines(&Js::Element(child.clone()), indent + 2));
        }
        out.push(format!("{}</{}>", pad(indent), element.tag));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(tag: &str, attrs: Vec<(&str, Attr)>, children: Vec<Element>) -> Element {
        Element {
            tag: tag.to_string(),
            attrs: attrs
                .into_iter()
                .map(|(name, attr)| (name.to_string(), attr))
                .collect(),
            children,
        }
    }

    #[test]
    fn an_element_is_one_line_while_it_fits_and_one_prop_a_line_after() {
        let short = element("Text", vec![("name", text_attr("label1"))], vec![]);
        assert_eq!(
            lines(&Js::Element(short.clone()), 2),
            ["  <Text name=\"label1\" />"]
        );
        let long = element(
            "DivContainer",
            vec![
                ("name", text_attr(&"n".repeat(60))),
                ("tabIndex", Attr::Value(Js::Raw("3".into()))),
                (
                    "appearance",
                    Attr::Value(Js::Element(element(
                        "Appearance",
                        vec![("class", text_attr(&"c".repeat(90)))],
                        vec![],
                    ))),
                ),
            ],
            vec![short],
        );
        let written = lines(&Js::Element(long), 0).join("\n");
        assert_eq!(
            written,
            format!(
                "<DivContainer\n  name=\"{}\"\n  tabIndex={{3}}\n  appearance={{\n    <Appearance\n      class=\"{}\"\n    />\n  }}\n>\n  <Text name=\"label1\" />\n</DivContainer>",
                "n".repeat(60),
                "c".repeat(90)
            )
        );
    }

    #[test]
    fn text_a_quoted_prop_would_change_is_an_expression() {
        assert_eq!(text_attr("a < b"), Attr::Text("a < b".into()));
        assert_eq!(
            text_attr("a &amp; \"b\""),
            Attr::Value(Js::Raw("\"a &amp; \\\"b\\\"\"".into()))
        );
        assert_eq!(
            text_attr("one\ntwo `x` ${y}"),
            Attr::Value(Js::Raw("`one\ntwo \\`x\\` \\${y}`".into()))
        );
        let lines = attr_lines("text", &text_attr("one\ntwo"), 4);
        assert_eq!(lines, ["    text={`one", "two`}"]);
    }

    #[test]
    fn lists_and_objects_break_one_entry_a_line() {
        let value = Js::Object(vec![
            ("en_US".to_string(), Js::Raw(json(&"a".repeat(60)))),
            ("pt-BR".to_string(), Js::Raw(json(&"b".repeat(60)))),
        ]);
        assert_eq!(
            lines(&value, 0),
            [
                "{".to_string(),
                format!("  en_US: \"{}\",", "a".repeat(60)),
                format!("  \"pt-BR\": \"{}\",", "b".repeat(60)),
                "}".to_string(),
            ]
        );
        assert_eq!(
            lines(
                &Js::Array(vec![Js::Raw("1".into()), Js::Raw("2".into())]),
                2
            ),
            ["  [1, 2]"]
        );
    }
}
