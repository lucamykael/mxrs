//! A JS-object-literal serializer — ports `js_literal`
//! (`nanoflow_program_compiler.rb:471-479`) exactly, including its
//! `"{ key: value, ... }"` spacing (space after `{`, before `}`, `", "`
//! between entries, `": "` after each JSON-quoted key) and its `Raw`
//! splice for the arrow-function thunks (`nanoflowCall`/
//! `javaScriptActionCall`'s `flow`/`action` fields).

use super::expression::{Expression, LiteralValue};

#[derive(Debug, Clone, PartialEq)]
pub enum JsValue {
    Null,
    Bool(bool),
    Str(String),
    Array(Vec<JsValue>),
    /// Insertion order matters — this is JS object-literal text, not a
    /// data structure with its own ordering semantics.
    Object(Vec<(String, JsValue)>),
    /// Spliced unescaped, byte-for-byte — used for arrow-function thunk
    /// expressions (`"() => name"`) that must appear unquoted.
    Raw(String),
}

impl JsValue {
    pub fn render(&self) -> String {
        match self {
            JsValue::Null => "null".to_string(),
            JsValue::Bool(b) => b.to_string(),
            JsValue::Str(s) => serde_json::to_string(s).expect("strings always serialize"),
            JsValue::Raw(s) => s.clone(),
            JsValue::Array(items) => {
                format!(
                    "[{}]",
                    items
                        .iter()
                        .map(JsValue::render)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            JsValue::Object(entries) => {
                if entries.is_empty() {
                    return "{  }".to_string();
                }
                let body = entries
                    .iter()
                    .map(|(k, v)| format!("{}: {}", serde_json::to_string(k).unwrap(), v.render()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{{ {body} }}")
            }
        }
    }

    pub fn object(entries: Vec<(&str, JsValue)>) -> JsValue {
        JsValue::Object(
            entries
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        )
    }
}

impl From<Expression> for JsValue {
    fn from(expression: Expression) -> Self {
        match expression {
            Expression::Literal(LiteralValue::Null) => JsValue::object(vec![
                ("type", JsValue::Str("literal".to_string())),
                ("value", JsValue::Null),
            ]),
            Expression::Literal(LiteralValue::Bool(b)) => JsValue::object(vec![
                ("type", JsValue::Str("literal".to_string())),
                ("value", JsValue::Bool(b)),
            ]),
            Expression::Literal(LiteralValue::Quoted(s))
            | Expression::Literal(LiteralValue::Opaque(s)) => JsValue::object(vec![
                ("type", JsValue::Str("literal".to_string())),
                ("value", JsValue::Str(s)),
            ]),
            Expression::LiteralNumeric(s) => JsValue::object(vec![
                ("type", JsValue::Str("literalNumeric".to_string())),
                ("value", JsValue::Str(s)),
            ]),
            Expression::Constant(name) => JsValue::object(vec![
                ("type", JsValue::Str("constant".to_string())),
                ("name", JsValue::Str(name)),
            ]),
            Expression::Token(name) => JsValue::object(vec![
                ("type", JsValue::Str("token".to_string())),
                ("name", JsValue::Str(name)),
            ]),
            Expression::Variable { name, path } => {
                let mut entries = vec![
                    ("type", JsValue::Str("variable".to_string())),
                    ("variable", JsValue::Str(name)),
                ];
                if let Some(path) = path {
                    entries.push(("path", JsValue::Str(path)));
                }
                JsValue::object(entries)
            }
            Expression::Conditional(condition, then, otherwise) => JsValue::object(vec![
                ("type", JsValue::Str("conditional".to_string())),
                ("condition", (*condition).into()),
                ("then", (*then).into()),
                ("else", (*otherwise).into()),
            ]),
            Expression::Function(name, parameters) => JsValue::object(vec![
                ("type", JsValue::Str("function".to_string())),
                ("name", JsValue::Str(name)),
                (
                    "parameters",
                    JsValue::Array(parameters.into_iter().map(JsValue::from).collect()),
                ),
            ]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_an_object_with_the_exact_ruby_spacing() {
        let value = JsValue::object(vec![
            ("type", JsValue::Str("return".to_string())),
            ("value", JsValue::Bool(true)),
        ]);
        assert_eq!(value.render(), r#"{ "type": "return", "value": true }"#);
    }

    #[test]
    fn renders_an_array_with_comma_space_separators() {
        let value = JsValue::Array(vec![
            JsValue::Str("a".to_string()),
            JsValue::Str("b".to_string()),
        ]);
        assert_eq!(value.render(), r#"["a", "b"]"#);
    }

    #[test]
    fn a_raw_value_splices_unescaped() {
        let value = JsValue::Raw("() => mxrbNanoflow_abc123".to_string());
        assert_eq!(value.render(), "() => mxrbNanoflow_abc123");
    }

    #[test]
    fn a_null_literal_expression_renders_to_a_literal_with_a_null_value() {
        let js: JsValue = super::super::expression::parse_expression("").0.into();
        assert_eq!(js.render(), r#"{ "type": "literal", "value": null }"#);
    }

    #[test]
    fn a_variable_expression_with_no_path_omits_the_path_key() {
        let js: JsValue = super::super::expression::parse_expression("$Order")
            .0
            .into();
        assert_eq!(
            js.render(),
            r#"{ "type": "variable", "variable": "Order" }"#
        );
    }
}
