//! The data a frontend file declares: `export default { ... } satisfies T;`
//! read as the JSON value it is.
//!
//! A declaration file is TypeScript that the frontend's own build checks
//! against its types, and that mxrs reads without running it. So what it
//! exports is data, written the way TypeScript writes data — objects,
//! arrays, strings, numbers, booleans, `null` — and anything that would
//! need running (a variable, a call, a spread) is refused with where it is.

use std::collections::HashMap;
use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    ArrayExpressionElement, Expression, ObjectPropertyKind, PropertyKey, Statement,
};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span};
use serde_json::{Map, Number, Value};

use crate::FrontendError;

/// What a file exports by default, and the line each part of it is on.
pub(crate) struct Declared {
    pub(crate) value: Value,
    /// The line of each value, by its path as `serde_path_to_error` writes
    /// it: `profiles[0].items[1].caption`, `.` for the whole.
    lines: HashMap<String, usize>,
}

impl Declared {
    /// The line the value at `path` — or the nearest part holding it — is
    /// on.
    pub(crate) fn line(&self, path: &str) -> Option<usize> {
        let mut path = path;
        loop {
            if let Some(line) = self.lines.get(path) {
                return Some(*line);
            }
            let cut = path.rfind(['.', '['])?;
            path = if cut == 0 { "." } else { &path[..cut] };
        }
    }
}

/// What the file at `path` exports by default, as data.
pub(crate) fn read_default_export(path: &Path) -> Result<Declared, FrontendError> {
    let source = std::fs::read_to_string(path).map_err(|source| FrontendError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(path).unwrap_or_else(|_| SourceType::ts());
    let parsed = Parser::new(&allocator, &source, source_type).parse();
    if let Some(error) = parsed.diagnostics.first() {
        return Err(crate::syntax_error(
            &path.display().to_string(),
            &source,
            error,
        ));
    }
    let reader = Reader {
        path,
        source: &source,
    };
    let mut exports = parsed
        .program
        .body
        .iter()
        .filter_map(|statement| match statement {
            Statement::ExportDefaultDeclaration(export) => Some(export),
            _ => None,
        });
    let Some(export) = exports.next() else {
        return Err(FrontendError::Syntax {
            path: path.display().to_string(),
            detail: "it has no `export default` declaring data".to_string(),
        });
    };
    if let Some(second) = exports.next() {
        return Err(reader.refuse(second.span, "one `export default`"));
    }
    let Some(expression) = export.declaration.as_expression() else {
        return Err(reader.refuse(export.span, "a default export that declares data"));
    };
    let mut lines = HashMap::new();
    let value = reader.value(expression, ".".to_string(), &mut lines)?;
    Ok(Declared { value, lines })
}

struct Reader<'a> {
    path: &'a Path,
    source: &'a str,
}

impl Reader<'_> {
    fn line(&self, span: Span) -> usize {
        let start = (span.start as usize).min(self.source.len());
        self.source[..start].matches('\n').count() + 1
    }

    fn refuse(&self, span: Span, expected: &str) -> FrontendError {
        let start = (span.start as usize).min(self.source.len());
        let end = (span.end as usize).clamp(start, self.source.len());
        let found: String = self.source[start..end].chars().take(60).collect();
        FrontendError::Unsupported {
            path: self.path.display().to_string(),
            line: self.line(span),
            expected: expected.to_string(),
            found,
        }
    }

    fn value(
        &self,
        expression: &Expression<'_>,
        path: String,
        lines: &mut HashMap<String, usize>,
    ) -> Result<Value, FrontendError> {
        let expression = expression.without_parentheses();
        lines.insert(path.clone(), self.line(expression.span()));
        Ok(match expression {
            Expression::StringLiteral(text) => {
                if text.lone_surrogates {
                    return Err(self.refuse(text.span, "text without a lone surrogate"));
                }
                Value::String(text.value.to_string())
            }
            Expression::TemplateLiteral(template) if template.expressions.is_empty() => {
                let mut text = String::new();
                for quasi in &template.quasis {
                    if quasi.lone_surrogates {
                        return Err(self.refuse(quasi.span, "text without a lone surrogate"));
                    }
                    match &quasi.value.cooked {
                        Some(cooked) => text.push_str(cooked),
                        None => return Err(self.refuse(quasi.span, "a valid escape")),
                    }
                }
                Value::String(text)
            }
            Expression::NumericLiteral(number) => {
                self.number(number.value, number.raw.as_deref(), number.span)?
            }
            Expression::UnaryExpression(unary)
                if unary.operator == oxc_ast::ast::UnaryOperator::UnaryNegation =>
            {
                match unary.argument.without_parentheses() {
                    Expression::NumericLiteral(number) => {
                        self.number(-number.value, number.raw.as_deref(), unary.span)?
                    }
                    other => return Err(self.refuse(other.span(), "a number")),
                }
            }
            Expression::BooleanLiteral(flag) => Value::Bool(flag.value),
            Expression::NullLiteral(_) => Value::Null,
            Expression::ArrayExpression(array) => {
                let mut items = Vec::with_capacity(array.elements.len());
                for (index, element) in array.elements.iter().enumerate() {
                    let element = match element {
                        ArrayExpressionElement::SpreadElement(spread) => {
                            return Err(self.refuse(spread.span, "a value, not a spread"));
                        }
                        ArrayExpressionElement::Elision(hole) => {
                            return Err(self.refuse(hole.span, "a value, not a hole"));
                        }
                        other => other
                            .as_expression()
                            .expect("an element that is neither a spread nor a hole"),
                    };
                    let path = if path == "." {
                        format!("[{index}]")
                    } else {
                        format!("{path}[{index}]")
                    };
                    items.push(self.value(element, path, lines)?);
                }
                Value::Array(items)
            }
            Expression::ObjectExpression(object) => {
                let mut map = Map::new();
                for property in &object.properties {
                    let ObjectPropertyKind::ObjectProperty(property) = property else {
                        return Err(self.refuse(property.span(), "a property, not a spread"));
                    };
                    if property.computed || property.method || property.shorthand {
                        return Err(self.refuse(property.span, "`key: value`"));
                    }
                    if let PropertyKey::StringLiteral(key) = &property.key
                        && key.lone_surrogates
                    {
                        return Err(self.refuse(key.span, "a key without a lone surrogate"));
                    }
                    let Some(key) = property.key.static_name() else {
                        return Err(self.refuse(property.key.span(), "a property name"));
                    };
                    if map.contains_key(key.as_ref()) {
                        return Err(self.refuse(property.key.span(), "each property once"));
                    }
                    let path = if path == "." {
                        key.to_string()
                    } else {
                        format!("{path}.{key}")
                    };
                    let value = self.value(&property.value, path, lines)?;
                    map.insert(key.to_string(), value);
                }
                Value::Object(map)
            }
            // `as const` and `satisfies T` say something to TypeScript only.
            Expression::TSAsExpression(cast) => self.value(&cast.expression, path, lines)?,
            Expression::TSSatisfiesExpression(check) => {
                self.value(&check.expression, path, lines)?
            }
            other => {
                return Err(self.refuse(
                    other.span(),
                    "data: a string, number, boolean, null, array or object",
                ));
            }
        })
    }

    /// A number as the integer it reads as when it is whole. From 2^53 on a
    /// whole number may no longer be the one written (`raw`), so one that
    /// is not is refused rather than rounded.
    fn number(&self, number: f64, raw: Option<&str>, span: Span) -> Result<Value, FrontendError> {
        const EXACT: f64 = 9_007_199_254_740_992.0;
        if number.fract() == 0.0 {
            // The digits as written, when they are plain decimal digits.
            let written = raw
                .map(|raw| raw.replace('_', ""))
                .and_then(|digits| digits.parse::<i128>().ok());
            #[allow(clippy::cast_possible_truncation)]
            let exact = number.abs() < EXACT || written == Some(number.abs() as i128);
            if !exact || number.abs() > EXACT {
                return Err(self.refuse(span, "a whole number within ±2^53"));
            }
            // Within ±2^53 a whole f64 is exactly the integer it reads as.
            #[allow(clippy::cast_possible_truncation)]
            return Ok(Value::Number(Number::from(number as i64)));
        }
        Number::from_f64(number)
            .map(Value::Number)
            .ok_or_else(|| self.refuse(span, "a finite number"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(source: &str) -> Result<Declared, FrontendError> {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("index.ts");
        std::fs::write(&path, source).unwrap();
        read_default_export(&path)
    }

    fn refusal(source: &str) -> String {
        read(source).err().unwrap().to_string()
    }

    #[test]
    fn a_default_export_is_read_as_the_data_it_declares() {
        let declared = read(
            "import type { Navigation } from \"@/types/navigation\";\n\nexport default {\n  profiles: [{ name: \"Responsive\", count: 3, ratio: -0.5, on: true, none: null, text: `a` }],\n  big: 9007199254740992,\n} as const satisfies Navigation;\n",
        )
        .unwrap();
        assert_eq!(
            declared.value,
            serde_json::json!({
                "profiles": [{ "name": "Responsive", "count": 3, "ratio": -0.5, "on": true, "none": null, "text": "a" }],
                "big": 9_007_199_254_740_992_i64,
            })
        );
        // Each part is found on its line, or on the line of what holds it.
        assert_eq!(declared.line("profiles[0].count"), Some(4));
        assert_eq!(declared.line("big"), Some(5));
        assert_eq!(declared.line("profiles[0].missing"), Some(4));
        assert_eq!(declared.line("."), Some(3));
    }

    #[test]
    fn what_would_need_running_is_refused_with_where_it_is() {
        let error = refusal("export default {\n  profiles: [title()],\n};\n");
        assert!(
            error.contains(":2:") && error.contains("title()"),
            "{error}"
        );
        assert!(refusal("export default { a: 1, a: 2 };\n").contains("each property once"));
        // A lone surrogate cannot be read without changing it.
        assert!(refusal("export default { a: \"\\uD800\" };\n").contains("lone surrogate"));
        assert!(refusal("export default { a: `\\uD800` };\n").contains("lone surrogate"));
        // Past 2^53 a whole number is not the one written.
        assert!(refusal("export default { a: 9007199254740993 };\n").contains("2^53"));
        // Only one default export says what the file declares.
        let error = refusal("export default { a: 1 };\nexport default { a: 2 };\n");
        assert!(
            error.contains("one `export default`") || error.contains("Duplicated export"),
            "{error}"
        );
    }
}
