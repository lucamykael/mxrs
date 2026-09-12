//! The nanoflow expression mini-language: a raw Mendix microflow-expression
//! **string** parsed by regex pattern matching in a fixed priority order —
//! ports `nanoflow_program_compiler.rb#expression` (lines 388-407)
//! verbatim, including its known ambiguities and gaps. The priority-ordered
//! regex structure *is* the specification here; this is deliberately not
//! rewritten as a "proper" recursive-descent parser, which would risk
//! silently fixing exactly the ambiguities below that must be reproduced.
//!
//! **Known gaps/ambiguities, reproduced not fixed** (flagged, not silently
//! carried over):
//! - The quoted-string rule does a blunt substring slice (`source[1..-2]`
//!   equivalent), not real unescaping — an embedded escaped quote inside a
//!   Mendix string literal won't round-trip correctly. Not something this
//!   pass can safely improve without diverging from `mxrb`'s own output.
//! - `binary_expression`'s left operand match is lazy (`(.+?)`), so it
//!   splits at the *first* operator keyword encountered scanning
//!   left-to-right, not by any real operator-precedence grammar. Recursion
//!   into the matched right-hand remainder often produces a reasonable
//!   tree anyway (the lazy match means the left capture itself can never
//!   contain an *earlier* operator — if one existed, it would have split
//!   there instead), but not always: a comparison operator can still win
//!   the top-level split ahead of a lower-precedence `and`/`or` that
//!   appears later in the string, e.g. `$a = $b and $c = $d` parses as
//!   `=($a, and($b, =($c, $d)))` instead of the presumably-intended
//!   `and(=($a,$b), =($c,$d))`. [`ExpressionDiagnostic::AmbiguousBinaryLeftOperand`]
//!   is a best-effort (not exhaustive) detector for exactly that shape —
//!   a non-`and`/`or` operator chosen while the remainder still contains
//!   one. The parse tree itself is left exactly as `mxrb` would produce
//!   it either way.
//! - The final fallback treats any string none of the other nine rules
//!   recognized as an opaque string literal, silently, with no error.
//!   [`LiteralValue::Opaque`] distinguishes this from a *real* quoted
//!   string literal at the type level (both render to the same JSON
//!   shape) — a diagnostics/tooling enabler, not a behavior change.

use regex::Regex;
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq)]
pub enum Expression {
    Literal(LiteralValue),
    /// Kept as the original text, not parsed to a real number — mirrors
    /// the Ruby avoiding float-precision loss on `Decimal`/`Long` values
    /// that must round-trip through JS as exact text.
    LiteralNumeric(String),
    Constant(String),
    Token(String),
    Variable {
        name: String,
        path: Option<String>,
    },
    Conditional(Box<Expression>, Box<Expression>, Box<Expression>),
    /// `and`/`or`/comparison operators and supported unary functions are
    /// all represented as generic named
    /// function calls — mirrors the Ruby's own shape, not a distinct
    /// binary-op node type.
    Function(String, Vec<Expression>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum LiteralValue {
    Null,
    Bool(bool),
    /// Matched a real quoted-string rule.
    Quoted(String),
    /// Fell through every recognized rule — see this module's doc comment.
    Opaque(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExpressionDiagnosticKind {
    AmbiguousBinaryLeftOperand,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExpressionDiagnostic {
    pub kind: ExpressionDiagnosticKind,
    pub source: String,
}

static CONDITIONAL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)\Aif\s+(.+?)\s+then\s+(.+?)\s+else\s+(.+)\z").unwrap());
static FUNCTION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)\A(not|isNew|isSynced|toString|trim|length)\((.*)\)\z").unwrap()
});
static BINARY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)\A(.+?)\s+(and|or|!=|=|>=|<=|>|<|\+)\s+(.+)\z").unwrap());
static VARIABLE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\A\$[A-Za-z_]\w*(?:/[A-Za-z_]\w*(?:\.[A-Za-z_]\w*)*)*\z").unwrap()
});
static NUMERIC_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A-?\d+(?:\.\d+)?\z").unwrap());
static ENUM_VALUE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A[A-Za-z_]\w*(?:\.[A-Za-z_]\w*){2,}\z").unwrap());
static TOKEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A\[%([A-Za-z_]\w*)%\]\z").unwrap());
static WORD_AND_OR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(and|or)\b").unwrap());

pub fn parse_expression(raw: &str) -> (Expression, Vec<ExpressionDiagnostic>) {
    let source = raw.trim();
    if source.is_empty() || source == "empty" {
        return (Expression::Literal(LiteralValue::Null), vec![]);
    }
    if let Some(caps) = CONDITIONAL_RE.captures(source) {
        let (condition, mut diagnostics) = parse_expression(&caps[1]);
        let (then, then_diagnostics) = parse_expression(&caps[2]);
        let (otherwise, else_diagnostics) = parse_expression(&caps[3]);
        diagnostics.extend(then_diagnostics);
        diagnostics.extend(else_diagnostics);
        return (
            Expression::Conditional(Box::new(condition), Box::new(then), Box::new(otherwise)),
            diagnostics,
        );
    }
    if let Some(caps) = FUNCTION_RE.captures(source) {
        let name = caps[1].to_string();
        let (inner, diagnostics) = parse_expression(&caps[2]);
        return (Expression::Function(name, vec![inner]), diagnostics);
    }
    if let Some(caps) = BINARY_RE.captures(source) {
        let left_source = caps[1].to_string();
        let right_source = caps[3].to_string();
        let operator = caps[2].to_lowercase();
        let (left, mut diagnostics) = parse_expression(&left_source);
        let (right, right_diagnostics) = parse_expression(&right_source);
        diagnostics.extend(right_diagnostics);
        // The lazy left match always stops at the *first* operator keyword
        // found scanning left-to-right, so `left_source` itself can never
        // contain an earlier one — the real ambiguity is the other
        // direction: a comparison operator (=, !=, <, ...) gets picked as
        // the top-level split *before* a lower-precedence `and`/`or` that
        // appears later in the string, e.g. `$a = $b and $c = $d` splits
        // as `=($a, and($b, =($c, $d)))` instead of the presumably-intended
        // `and(=($a,$b), =($c,$d))`. Flagged when that specific shape
        // occurs; the parse tree itself is left exactly as `mxrb` would
        // produce it.
        if !matches!(operator.as_str(), "and" | "or") && WORD_AND_OR_RE.is_match(&right_source) {
            diagnostics.push(ExpressionDiagnostic {
                kind: ExpressionDiagnosticKind::AmbiguousBinaryLeftOperand,
                source: source.to_string(),
            });
        }
        let name = operator;
        return (Expression::Function(name, vec![left, right]), diagnostics);
    }
    if VARIABLE_RE.is_match(source) {
        let rest = &source[1..];
        let mut parts = rest.split('/');
        let name = parts.next().unwrap_or_default().to_string();
        let path: Vec<&str> = parts.collect();
        let path = (!path.is_empty()).then(|| path.join("/"));
        return (Expression::Variable { name, path }, vec![]);
    }
    if let Some(rest) = source.strip_prefix('@') {
        return (Expression::Constant(rest.to_string()), vec![]);
    }
    if let Some(caps) = TOKEN_RE.captures(source) {
        let name = caps[1].to_string();
        let mut characters = name.chars();
        let normalized = characters
            .next()
            .map(|first| first.to_ascii_lowercase().to_string() + characters.as_str())
            .unwrap_or_default();
        return (Expression::Token(normalized), vec![]);
    }
    if source == "true" || source == "false" {
        return (
            Expression::Literal(LiteralValue::Bool(source == "true")),
            vec![],
        );
    }
    if is_quoted(source) {
        let inner = &source[1..source.len() - 1];
        return (
            Expression::Literal(LiteralValue::Quoted(inner.to_string())),
            vec![],
        );
    }
    if NUMERIC_RE.is_match(source) {
        return (Expression::LiteralNumeric(source.to_string()), vec![]);
    }
    if ENUM_VALUE_RE.is_match(source) {
        return (
            Expression::Literal(LiteralValue::Quoted(
                source.rsplit('.').next().unwrap_or_default().to_string(),
            )),
            vec![],
        );
    }
    (
        Expression::Literal(LiteralValue::Opaque(source.to_string())),
        vec![],
    )
}

fn is_quoted(value: &str) -> bool {
    (value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2)
        || (value.starts_with('"') && value.ends_with('"') && value.len() >= 2)
}

/// The leading `$Identifier` of a variable-shaped expression, if any —
/// mirrors `expression_kind`'s own regex extraction
/// (`nanoflow_program_compiler.rb:409-412`), kept separate from
/// [`parse_expression`] since it's applied to the *raw* string
/// independently of the full parse (a variable reference nested inside a
/// larger expression, e.g. `$Order/Total`, still needs its kind resolved).
pub fn leading_variable_name(raw: &str) -> Option<String> {
    static LEADING_VARIABLE_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\A\$([A-Za-z_]\w*)").unwrap());
    LEADING_VARIABLE_RE
        .captures(raw.trim())
        .map(|caps| caps[1].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_and_empty_keyword_are_null_literals() {
        assert_eq!(
            parse_expression("").0,
            Expression::Literal(LiteralValue::Null)
        );
        assert_eq!(
            parse_expression("empty").0,
            Expression::Literal(LiteralValue::Null)
        );
    }

    #[test]
    fn parses_a_conditional_expression() {
        let (expr, _) = parse_expression("if $a then $b else $c");
        assert!(matches!(expr, Expression::Conditional(_, _, _)));
    }

    #[test]
    fn parses_supported_unary_functions() {
        for name in ["not", "isNew", "isSynced", "toString", "trim", "length"] {
            let (expr, _) = parse_expression(&format!("{name}($a)"));
            assert_eq!(
                expr,
                Expression::Function(
                    name.to_string(),
                    vec![Expression::Variable {
                        name: "a".to_string(),
                        path: None
                    }],
                )
            );
        }
    }

    #[test]
    fn parses_a_binary_comparison_with_a_lowercased_operator_name() {
        let (expr, diagnostics) = parse_expression("$a = $b");
        assert_eq!(
            expr,
            Expression::Function(
                "=".to_string(),
                vec![
                    Expression::Variable {
                        name: "a".to_string(),
                        path: None
                    },
                    Expression::Variable {
                        name: "b".to_string(),
                        path: None
                    },
                ],
            )
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn flags_a_comparison_that_wins_the_split_ahead_of_a_later_and_or() {
        // First operator found scanning left-to-right is "=" (before
        // "and" ever appears), so it wins the top-level split even though
        // a human reading this almost certainly intends
        // `($a = $b) and $c`, not `$a = ($b and $c)`.
        let (expr, diagnostics) = parse_expression("$a = $b and $c");
        assert_eq!(
            expr,
            Expression::Function(
                "=".to_string(),
                vec![
                    Expression::Variable {
                        name: "a".to_string(),
                        path: None
                    },
                    Expression::Function(
                        "and".to_string(),
                        vec![
                            Expression::Variable {
                                name: "b".to_string(),
                                path: None
                            },
                            Expression::Variable {
                                name: "c".to_string(),
                                path: None
                            },
                        ],
                    ),
                ],
            ),
            "reproduces mxrb's actual (mis-)parse verbatim"
        );
        assert!(
            diagnostics
                .iter()
                .any(|d| d.kind == ExpressionDiagnosticKind::AmbiguousBinaryLeftOperand)
        );
    }

    #[test]
    fn does_not_flag_a_correctly_ordered_and_then_comparisons() {
        // "and" is found first, so it correctly becomes the top-level
        // split; each side then recurses into its own real comparison.
        // No ambiguity to flag here.
        let (_, diagnostics) = parse_expression("$a and $b = $c");
        assert!(
            diagnostics
                .iter()
                .all(|d| d.kind != ExpressionDiagnosticKind::AmbiguousBinaryLeftOperand)
        );
    }

    #[test]
    fn parses_a_variable_with_a_path() {
        let (expr, _) = parse_expression("$Order/Customer/Name");
        assert_eq!(
            expr,
            Expression::Variable {
                name: "Order".to_string(),
                path: Some("Customer/Name".to_string()),
            }
        );
    }

    #[test]
    fn parses_qualified_association_segments_in_a_variable_path() {
        let (expr, _) =
            parse_expression("$Order/Sales.Order_Customer/Sales.Customer/AccountManager");
        assert_eq!(
            expr,
            Expression::Variable {
                name: "Order".to_string(),
                path: Some("Sales.Order_Customer/Sales.Customer/AccountManager".to_string()),
            }
        );
    }

    #[test]
    fn parses_a_constant() {
        let (expr, _) = parse_expression("@Module.MyConstant");
        assert_eq!(expr, Expression::Constant("Module.MyConstant".to_string()));
    }

    #[test]
    fn parses_runtime_tokens_and_enumeration_values() {
        assert_eq!(
            parse_expression("[%CurrentDateTime%]").0,
            Expression::Token("currentDateTime".to_string())
        );
        assert_eq!(
            parse_expression("Sales.OrderStatus.Open").0,
            Expression::Literal(LiteralValue::Quoted("Open".to_string()))
        );
    }

    #[test]
    fn parses_boolean_keywords() {
        assert_eq!(
            parse_expression("true").0,
            Expression::Literal(LiteralValue::Bool(true))
        );
        assert_eq!(
            parse_expression("false").0,
            Expression::Literal(LiteralValue::Bool(false))
        );
    }

    #[test]
    fn parses_a_quoted_string_without_unescaping() {
        let (expr, _) = parse_expression("'hello'");
        assert_eq!(
            expr,
            Expression::Literal(LiteralValue::Quoted("hello".to_string()))
        );
    }

    #[test]
    fn parses_a_numeric_literal_as_text() {
        let (expr, _) = parse_expression("42");
        assert_eq!(expr, Expression::LiteralNumeric("42".to_string()));
        let (expr, _) = parse_expression("-3.5");
        assert_eq!(expr, Expression::LiteralNumeric("-3.5".to_string()));
    }

    #[test]
    fn an_unrecognized_expression_is_an_opaque_literal_not_an_error() {
        let (expr, _) = parse_expression("something weird here");
        assert_eq!(
            expr,
            Expression::Literal(LiteralValue::Opaque("something weird here".to_string()))
        );
    }

    #[test]
    fn leading_variable_name_extracts_the_identifier() {
        assert_eq!(
            leading_variable_name("$Order/Total"),
            Some("Order".to_string())
        );
        assert_eq!(leading_variable_name("'literal'"), None);
    }
}
