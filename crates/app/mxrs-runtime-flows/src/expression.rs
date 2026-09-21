//! The Mendix expression engine.
//!
//! Ports `Native::Expression` from mxrb: a deliberately small evaluator —
//! unsupported syntax is rejected rather than guessed, preserving
//! deterministic execution semantics. Like the oracle, `and`/`or` evaluate
//! both sides (no short-circuit) and combine truthiness, and every internal
//! type error surfaces as one `unsupported Mendix expression` diagnostic
//! carrying the original source.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mxrs_runtime::ObjectValue;

use crate::value::{FlowValue, ObjectRef, Variables};
use crate::{FlowError, datetime};

/// Reads an object member for `$variable/Member` references. mxrb variables
/// hold live records; this port holds references and reads through whatever
/// the engine supplies (the store during flow execution, nothing for
/// standalone evaluation).
pub trait MemberSource {
    fn object_member(&self, reference: &ObjectRef, member: &str) -> Result<FlowValue, FlowError>;
}

/// Standalone evaluation context: any object-member read is an error.
pub struct NoObjects;

impl MemberSource for NoObjects {
    fn object_member(&self, reference: &ObjectRef, _member: &str) -> Result<FlowValue, FlowError> {
        Err(FlowError::native(format!(
            "${} is not an object",
            reference.entity
        )))
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(FlowValue),
    Str(String),
    Variable(String),
    Identifier(String),
    Operator(&'static str),
    DateTimeNow,
    LeftParenthesis,
    RightParenthesis,
    Comma,
}

/// Internal failure while parsing/evaluating; mapped to the single public
/// `unsupported Mendix expression: <source>` message, exactly like mxrb
/// rescuing `ArgumentError`/`TypeError`/`KeyError`. `Fatal` carries errors
/// that must pass through unwrapped (unknown variable, store failures).
enum ExprFailure {
    Unsupported,
    Fatal(FlowError),
}

impl From<FlowError> for ExprFailure {
    fn from(error: FlowError) -> Self {
        ExprFailure::Fatal(error)
    }
}

type ExprResult = Result<FlowValue, ExprFailure>;

#[derive(Default)]
pub struct Expression {
    cache: Mutex<BTreeMap<String, Arc<Vec<Token>>>>,
}

impl Expression {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn evaluate(
        &self,
        source: &str,
        variables: &Variables,
        node: Option<&ObjectValue>,
        members: &dyn MemberSource,
    ) -> Result<FlowValue, FlowError> {
        let text = source.trim();
        if text.is_empty() {
            return Ok(FlowValue::Empty);
        }
        let tokens = self.tokens(text).map_err(|failure| match failure {
            ExprFailure::Fatal(error) => error,
            ExprFailure::Unsupported => FlowError::unsupported_expression(format!("{source:?}")),
        })?;
        let mut parser = Parser {
            tokens: &tokens,
            index: 0,
            variables,
            node,
            members,
        };
        let outcome = parser.parse();
        match outcome {
            Ok(value) => Ok(value),
            Err(ExprFailure::Fatal(error)) => Err(error),
            Err(ExprFailure::Unsupported) => {
                Err(FlowError::unsupported_expression(format!("{source:?}")))
            }
        }
    }

    fn tokens(&self, text: &str) -> Result<Arc<Vec<Token>>, ExprFailure> {
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(tokens) = cache.get(text) {
            return Ok(tokens.clone());
        }
        let tokens = Arc::new(Lexer::new(text).tokens()?);
        cache.insert(text.to_string(), tokens.clone());
        Ok(tokens)
    }
}

/// mxrb's `resolve_variable`: `$name` or `$name/Member.Path` (the member's
/// final dotted segment names the attribute).
fn resolve_variable(
    text: &str,
    variables: &Variables,
    members: &dyn MemberSource,
) -> Result<FlowValue, FlowError> {
    let reference = text.trim_start_matches('$');
    let (name, member) = match reference.split_once('/') {
        Some((name, member)) => (name, Some(member)),
        None => (reference, None),
    };
    let value = variables
        .get(name)
        .ok_or_else(|| FlowError::native(format!("unknown variable ${name}")))?;
    let Some(member) = member else {
        return Ok(value.clone());
    };
    let FlowValue::Object(object) = value else {
        return Err(FlowError::native(format!("${name} is not an object")));
    };
    let attribute = member.rsplit('.').next().unwrap_or(member);
    members.object_member(object, attribute)
}

struct Parser<'a> {
    tokens: &'a [Token],
    index: usize,
    variables: &'a Variables,
    node: Option<&'a ObjectValue>,
    members: &'a dyn MemberSource,
}

impl Parser<'_> {
    fn parse(&mut self) -> ExprResult {
        let value = self.parse_or()?;
        if self.peek().is_some() {
            return Err(ExprFailure::Unsupported);
        }
        Ok(value)
    }

    fn parse_or(&mut self) -> ExprResult {
        let mut left = self.parse_and()?;
        while self.accept_word("or") {
            let right = self.parse_and()?;
            left = FlowValue::Bool(left.truthy() || right.truthy());
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> ExprResult {
        let mut left = self.parse_comparison()?;
        while self.accept_word("and") {
            let right = self.parse_comparison()?;
            left = FlowValue::Bool(left.truthy() && right.truthy());
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> ExprResult {
        let mut left = self.parse_addition()?;
        while let Some(Token::Operator(operator)) = self.peek() {
            let operator = *operator;
            if !matches!(operator, "=" | "!=" | ">" | "<" | ">=" | "<=") {
                break;
            }
            self.advance();
            let right = self.parse_addition()?;
            left = FlowValue::Bool(match operator {
                "=" => left.equals(&right),
                "!=" => !left.equals(&right),
                ordering => {
                    let comparison = left.compare(&right).map_err(|_| ExprFailure::Unsupported)?;
                    match ordering {
                        ">" => comparison.is_gt(),
                        "<" => comparison.is_lt(),
                        ">=" => comparison.is_ge(),
                        "<=" => comparison.is_le(),
                        _ => unreachable!("comparison operators are exhaustive"),
                    }
                }
            });
        }
        Ok(left)
    }

    fn parse_addition(&mut self) -> ExprResult {
        let mut left = self.parse_multiplication()?;
        while let Some(Token::Operator(operator @ ("+" | "-"))) = self.peek() {
            let operator = *operator;
            self.advance();
            let right = self.parse_multiplication()?;
            left = if operator == "+" {
                left.add(&right)
            } else {
                left.subtract(&right)
            }
            .map_err(|_| ExprFailure::Unsupported)?;
        }
        Ok(left)
    }

    fn parse_multiplication(&mut self) -> ExprResult {
        let mut left = self.parse_unary()?;
        while let Some(Token::Operator(operator @ ("*" | "/"))) = self.peek() {
            let operator = *operator;
            self.advance();
            let right = self.parse_unary()?;
            left = if operator == "*" {
                left.multiply(&right)
            } else {
                left.divide(&right)
            }
            .map_err(|_| ExprFailure::Unsupported)?;
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> ExprResult {
        if self.accept_word("not") {
            let value = self.parse_unary()?;
            return Ok(FlowValue::Bool(!value.truthy()));
        }
        if matches!(self.peek(), Some(Token::Operator("-"))) {
            self.advance();
            let value = self.parse_unary()?;
            return value.negate().map_err(|_| ExprFailure::Unsupported);
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> ExprResult {
        match self.peek().cloned() {
            Some(Token::Number(value)) => {
                self.advance();
                Ok(value)
            }
            Some(Token::Str(value)) => {
                self.advance();
                Ok(FlowValue::String(value))
            }
            Some(Token::DateTimeNow) => {
                self.advance();
                Ok(FlowValue::now())
            }
            Some(Token::Variable(text)) => {
                self.advance();
                Ok(resolve_variable(&text, self.variables, self.members)?)
            }
            Some(Token::LeftParenthesis) => {
                self.advance();
                let value = self.parse_or()?;
                self.consume(&Token::RightParenthesis)?;
                Ok(value)
            }
            Some(Token::Identifier(name)) => {
                self.advance();
                self.identifier(name)
            }
            _ => Err(ExprFailure::Unsupported),
        }
    }

    fn identifier(&mut self, name: String) -> ExprResult {
        if !matches!(self.peek(), Some(Token::LeftParenthesis)) {
            return self.resolve_identifier(&name);
        }
        self.advance();
        let mut arguments = Vec::new();
        if !self.accept(&Token::RightParenthesis) {
            loop {
                arguments.push(self.parse_or()?);
                if self.accept(&Token::RightParenthesis) {
                    break;
                }
                self.consume(&Token::Comma)?;
            }
        }
        invoke(&name, &arguments)
    }

    /// mxrb's `resolve_identifier`, in its exact order: keyword literals, a
    /// node member that exists, an `A.B.C` enum literal, then a node member
    /// that may be absent (nil), then rejection.
    fn resolve_identifier(&self, text: &str) -> ExprResult {
        if text.eq_ignore_ascii_case("true") {
            return Ok(FlowValue::Bool(true));
        }
        if text.eq_ignore_ascii_case("false") {
            return Ok(FlowValue::Bool(false));
        }
        if text.eq_ignore_ascii_case("empty") {
            return Ok(FlowValue::Empty);
        }
        let bare = bare_reference(text);
        if let Some(node) = self.node
            && bare
        {
            let member = final_segment(text);
            if let Some(value) = node.members.get(member) {
                return Ok(FlowValue::from_member(value));
            }
        }
        if is_enum_literal(text) {
            return Ok(FlowValue::String(text.to_string()));
        }
        if let Some(node) = self.node
            && bare
        {
            let member = final_segment(text);
            return Ok(node
                .members
                .get(member)
                .map(FlowValue::from_member)
                .unwrap_or(FlowValue::Empty));
        }
        Err(ExprFailure::Unsupported)
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.index)
    }

    fn advance(&mut self) {
        self.index += 1;
    }

    fn accept(&mut self, token: &Token) -> bool {
        if self.peek() == Some(token) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn accept_word(&mut self, word: &str) -> bool {
        if let Some(Token::Identifier(name)) = self.peek()
            && name.eq_ignore_ascii_case(word)
        {
            self.advance();
            return true;
        }
        false
    }

    fn consume(&mut self, token: &Token) -> Result<(), ExprFailure> {
        if self.accept(token) {
            Ok(())
        } else {
            Err(ExprFailure::Unsupported)
        }
    }
}

/// `Name` or `Clinic.Animal/Name` — an attribute reference inside an XPath
/// predicate, resolved against the current candidate object.
fn bare_reference(text: &str) -> bool {
    let mut parts = text.splitn(2, '/');
    let head = parts.next().unwrap_or_default();
    let head_ok = !head.is_empty()
        && head.split('.').all(|segment| {
            let mut characters = segment.chars();
            characters
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
                && characters.all(|rest| rest.is_ascii_alphanumeric() || rest == '_')
        });
    let tail_ok = match parts.next() {
        None => true,
        Some(tail) => {
            let mut characters = tail.chars();
            characters
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
                && characters.all(|rest| rest.is_ascii_alphanumeric() || rest == '_' || rest == '.')
        }
    };
    head_ok && tail_ok
}

fn is_enum_literal(text: &str) -> bool {
    let segments: Vec<&str> = text.split('.').collect();
    segments.len() == 3
        && segments.iter().all(|segment| {
            let mut characters = segment.chars();
            characters
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
                && characters.all(|rest| rest.is_ascii_alphanumeric() || rest == '_')
        })
}

fn final_segment(text: &str) -> &str {
    let after_slash = text.rsplit('/').next().unwrap_or(text);
    after_slash.rsplit('.').next().unwrap_or(after_slash)
}

/// mxrb's `invoke`: the built-in function table.
fn invoke(name: &str, arguments: &[FlowValue]) -> ExprResult {
    let argument = |index: usize| arguments.get(index).ok_or(ExprFailure::Unsupported);
    Ok(match name.to_ascii_lowercase().as_str() {
        "tostring" => FlowValue::String(argument(0)?.mendix_string()),
        "parseinteger" => match argument(0)? {
            FlowValue::Int(value) => FlowValue::Int(*value),
            FlowValue::String(value) => FlowValue::Int(
                value
                    .trim()
                    .parse::<i64>()
                    .map_err(|_| ExprFailure::Unsupported)?,
            ),
            _ => return Err(ExprFailure::Unsupported),
        },
        "parsedecimal" => match argument(0)? {
            FlowValue::Int(value) => FlowValue::Float(*value as f64),
            FlowValue::Float(value) => FlowValue::Float(*value),
            FlowValue::String(value) => FlowValue::Float(
                value
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| ExprFailure::Unsupported)?,
            ),
            _ => return Err(ExprFailure::Unsupported),
        },
        "round" => match argument(0)? {
            FlowValue::Int(value) => FlowValue::Int(*value),
            // Ruby rounds half away from zero.
            FlowValue::Float(value) => FlowValue::Int(value.round() as i64),
            _ => return Err(ExprFailure::Unsupported),
        },
        "random" => FlowValue::Float(pseudo_random()),
        "substring" => {
            let text = argument(0)?.mendix_string();
            let start = match argument(1)? {
                FlowValue::Int(value) => *value,
                _ => return Err(ExprFailure::Unsupported),
            };
            let length = match arguments.get(2) {
                None => None,
                Some(FlowValue::Int(value)) => Some(*value),
                Some(_) => return Err(ExprFailure::Unsupported),
            };
            FlowValue::String(ruby_slice(&text, start, length))
        }
        "find" => {
            let haystack = argument(0)?.mendix_string();
            let needle = argument(1)?.mendix_string();
            FlowValue::Int(
                haystack
                    .find(&needle)
                    .map(|byte| haystack[..byte].chars().count() as i64)
                    .unwrap_or(-1),
            )
        }
        "formatdatetime" => {
            let FlowValue::DateTime(seconds) = argument(0)? else {
                return Err(ExprFailure::Unsupported);
            };
            let pattern = argument(1)?.mendix_string();
            FlowValue::String(datetime::format(*seconds, &pattern))
        }
        "contains" => FlowValue::Bool(
            argument(0)?
                .mendix_string()
                .contains(&argument(1)?.mendix_string()),
        ),
        "startswith" => FlowValue::Bool(
            argument(0)?
                .mendix_string()
                .starts_with(&argument(1)?.mendix_string()),
        ),
        "endswith" => FlowValue::Bool(
            argument(0)?
                .mendix_string()
                .ends_with(&argument(1)?.mendix_string()),
        ),
        _ => return Err(ExprFailure::Unsupported),
    })
}

/// Ruby `String#slice(start[, length])` on characters, with negative starts
/// counting from the end; `nil` results collapse to `""` via `to_s`.
fn ruby_slice(text: &str, start: i64, length: Option<i64>) -> String {
    let characters: Vec<char> = text.chars().collect();
    let size = characters.len() as i64;
    let begin = if start < 0 { size + start } else { start };
    if begin < 0 || begin > size {
        return String::new();
    }
    let begin = begin as usize;
    match length {
        None => characters[begin..].iter().collect(),
        Some(length) if length < 0 => String::new(),
        Some(length) => characters[begin..(begin + length as usize).min(characters.len())]
            .iter()
            .collect(),
    }
}

/// `random()` without a rand dependency: uuid v4's 122 random bits.
fn pseudo_random() -> f64 {
    let bytes = uuid::Uuid::new_v4().into_bytes();
    let mut value: u64 = 0;
    for byte in &bytes[..8] {
        value = (value << 8) | u64::from(*byte);
    }
    (value >> 11) as f64 / (1u64 << 53) as f64
}

struct Lexer<'a> {
    rest: &'a str,
}

impl<'a> Lexer<'a> {
    fn new(text: &'a str) -> Self {
        Self { rest: text }
    }

    fn tokens(mut self) -> Result<Vec<Token>, ExprFailure> {
        let mut result = Vec::new();
        loop {
            self.rest = self.rest.trim_start();
            if self.rest.is_empty() {
                return Ok(result);
            }
            result.push(self.next_token()?);
        }
    }

    fn next_token(&mut self) -> Result<Token, ExprFailure> {
        let lowered = self.rest.to_ascii_lowercase();
        if lowered.starts_with("[%currentdatetime%]") {
            self.rest = &self.rest["[%CurrentDateTime%]".len()..];
            return Ok(Token::DateTimeNow);
        }
        let mut characters = self.rest.chars();
        let first = characters.next().expect("caller checked non-empty");
        if first == '\'' {
            return self.scan_string();
        }
        if first.is_ascii_digit() {
            return Ok(self.scan_number());
        }
        if first == '$' {
            // mxrb: `\$[A-Za-z_]\w*(?:/[A-Za-z_][\w.]*)?` — one optional
            // member path after a slash; dots only inside the member.
            let name = take_while(&self.rest[1..], |c, index| {
                if index == 0 {
                    c.is_ascii_alphabetic() || c == '_'
                } else {
                    c.is_ascii_alphanumeric() || c == '_'
                }
            });
            if name.is_empty() {
                return Err(ExprFailure::Unsupported);
            }
            let mut length = 1 + name.len();
            if let Some(member_text) = self.rest[length..].strip_prefix('/') {
                let member = take_while(member_text, |c, index| {
                    if index == 0 {
                        c.is_ascii_alphabetic() || c == '_'
                    } else {
                        c.is_ascii_alphanumeric() || c == '_' || c == '.'
                    }
                });
                if !member.is_empty() {
                    length += 1 + member.len();
                }
            }
            let token = &self.rest[..length];
            self.rest = &self.rest[length..];
            return Ok(Token::Variable(token.to_string()));
        }
        if first.is_ascii_alphabetic() || first == '_' {
            let taken = take_while(self.rest, |c, _| {
                c.is_ascii_alphanumeric() || c == '_' || c == '.'
            });
            self.rest = &self.rest[taken.len()..];
            return Ok(Token::Identifier(taken.to_string()));
        }
        for operator in ["!=", ">=", "<=", "=", ">", "<", "+", "-", "*", "/"] {
            if let Some(remaining) = self.rest.strip_prefix(operator) {
                self.rest = remaining;
                return Ok(Token::Operator(match operator {
                    "!=" => "!=",
                    ">=" => ">=",
                    "<=" => "<=",
                    "=" => "=",
                    ">" => ">",
                    "<" => "<",
                    "+" => "+",
                    "-" => "-",
                    "*" => "*",
                    "/" => "/",
                    _ => unreachable!(),
                }));
            }
        }
        let token = match first {
            '(' => Token::LeftParenthesis,
            ')' => Token::RightParenthesis,
            ',' => Token::Comma,
            _ => return Err(ExprFailure::Unsupported),
        };
        self.rest = characters.as_str();
        Ok(token)
    }

    fn scan_string(&mut self) -> Result<Token, ExprFailure> {
        let mut value = String::new();
        let mut characters = self.rest.char_indices().skip(1).peekable();
        while let Some((index, character)) = characters.next() {
            if character != '\'' {
                value.push(character);
                continue;
            }
            if characters.peek().is_some_and(|(_, next)| *next == '\'') {
                characters.next();
                value.push('\'');
                continue;
            }
            self.rest = &self.rest[index + 1..];
            return Ok(Token::Str(value));
        }
        // Unterminated string: mxrb raises ArgumentError.
        Err(ExprFailure::Unsupported)
    }

    fn scan_number(&mut self) -> Token {
        let digits = take_while(self.rest, |c, _| c.is_ascii_digit());
        let mut end = digits.len();
        let after = &self.rest[end..];
        if let Some(fraction) = after.strip_prefix('.') {
            let decimals = take_while(fraction, |c, _| c.is_ascii_digit());
            if !decimals.is_empty() {
                end += 1 + decimals.len();
            }
        }
        let text = &self.rest[..end];
        self.rest = &self.rest[end..];
        if text.contains('.') {
            Token::Number(FlowValue::Float(text.parse().unwrap_or(0.0)))
        } else {
            Token::Number(
                text.parse::<i64>()
                    .map(FlowValue::Int)
                    .unwrap_or(FlowValue::Float(text.parse().unwrap_or(0.0))),
            )
        }
    }
}

fn take_while(text: &str, keep: impl Fn(char, usize) -> bool) -> &str {
    let mut end = 0;
    for (index, character) in text.char_indices() {
        if !keep(character, index) {
            break;
        }
        end = index + character.len_utf8();
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(source: &str) -> FlowValue {
        Expression::new()
            .evaluate(source, &Variables::new(), None, &NoObjects)
            .unwrap()
    }

    fn eval_with(source: &str, variables: &Variables) -> FlowValue {
        Expression::new()
            .evaluate(source, variables, None, &NoObjects)
            .unwrap()
    }

    #[test]
    fn literals_arithmetic_precedence_and_grouping() {
        assert_eq!(eval("1 + 2 * 3"), FlowValue::Int(7));
        assert_eq!(eval("(1 + 2) * 3"), FlowValue::Int(9));
        assert_eq!(eval("10 / 4"), FlowValue::Int(2));
        assert_eq!(eval("10.0 / 4"), FlowValue::Float(2.5));
        assert_eq!(eval("-3 + 1"), FlowValue::Int(-2));
        assert_eq!(eval("'a''b'"), FlowValue::String("a'b".into()));
        assert_eq!(eval(""), FlowValue::Empty);
        assert_eq!(eval("empty"), FlowValue::Empty);
        assert_eq!(eval("TRUE"), FlowValue::Bool(true));
    }

    #[test]
    fn comparisons_boolean_logic_and_unsupported_syntax() {
        assert_eq!(eval("1 < 2 and 2 < 3"), FlowValue::Bool(true));
        assert_eq!(eval("1 > 2 or 3 = 3"), FlowValue::Bool(true));
        assert_eq!(eval("not (1 = 1)"), FlowValue::Bool(false));
        assert_eq!(eval("1 != 1.0"), FlowValue::Bool(false));
        assert_eq!(eval("'b' > 'a'"), FlowValue::Bool(true));
        let error = Expression::new()
            .evaluate("1 ~ 2", &Variables::new(), None, &NoObjects)
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "unsupported Mendix expression: \"1 ~ 2\""
        );
        assert!(
            Expression::new()
                .evaluate("'a' > 1", &Variables::new(), None, &NoObjects)
                .is_err()
        );
    }

    #[test]
    fn variables_members_and_unknowns() {
        let mut variables = Variables::new();
        variables.insert("count".into(), FlowValue::Int(41));
        assert_eq!(eval_with("$count + 1", &variables), FlowValue::Int(42));
        let error = Expression::new()
            .evaluate("$missing", &variables, None, &NoObjects)
            .unwrap_err();
        assert_eq!(error.to_string(), "unknown variable $missing");
        variables.insert(
            "order".into(),
            FlowValue::Object(ObjectRef {
                entity: "Sales.Order".into(),
                id: "1".into(),
            }),
        );
        struct Fixed;
        impl MemberSource for Fixed {
            fn object_member(
                &self,
                _reference: &ObjectRef,
                member: &str,
            ) -> Result<FlowValue, FlowError> {
                assert_eq!(member, "Total");
                Ok(FlowValue::Float(9.5))
            }
        }
        let value = Expression::new()
            .evaluate("$order/Sales.Order.Total > 9", &variables, None, &Fixed)
            .unwrap();
        assert_eq!(value, FlowValue::Bool(true));
    }

    #[test]
    fn builtin_functions_match_the_oracle_table() {
        assert_eq!(eval("toString(2.0)"), FlowValue::String("2.0".into()));
        assert_eq!(eval("parseInteger('12')"), FlowValue::Int(12));
        assert_eq!(eval("parseDecimal('1.5')"), FlowValue::Float(1.5));
        assert_eq!(eval("round(2.5)"), FlowValue::Int(3));
        assert_eq!(eval("round(-2.5)"), FlowValue::Int(-3));
        assert_eq!(
            eval("substring('hello', 1, 3)"),
            FlowValue::String("ell".into())
        );
        assert_eq!(
            eval("substring('hello', 3)"),
            FlowValue::String("lo".into())
        );
        assert_eq!(
            eval("substring('hello', -2)"),
            FlowValue::String("lo".into())
        );
        assert_eq!(eval("find('hello', 'll')"), FlowValue::Int(2));
        assert_eq!(eval("find('hello', 'x')"), FlowValue::Int(-1));
        assert_eq!(eval("contains('hello', 'ell')"), FlowValue::Bool(true));
        assert_eq!(eval("startsWith('hello', 'he')"), FlowValue::Bool(true));
        assert_eq!(eval("endsWith('hello', 'lo')"), FlowValue::Bool(true));
        assert!(matches!(eval("random()"), FlowValue::Float(value) if (0.0..1.0).contains(&value)));
        assert!(matches!(
            eval("formatDateTime([%CurrentDateTime%], 'yyyy')"),
            FlowValue::String(year) if year.len() == 4
        ));
        assert!(
            Expression::new()
                .evaluate("parseInteger('x')", &Variables::new(), None, &NoObjects)
                .is_err()
        );
    }

    #[test]
    fn enum_literals_and_node_members_resolve_in_oracle_order() {
        let mut node = ObjectValue {
            entity: "Sales.Order".into(),
            id: "1".into(),
            members: BTreeMap::new(),
        };
        node.members
            .insert("Name".into(), serde_json::Value::String("Ada".into()));
        let expression = Expression::new();
        assert_eq!(
            expression
                .evaluate("Name = 'Ada'", &Variables::new(), Some(&node), &NoObjects)
                .unwrap(),
            FlowValue::Bool(true)
        );
        assert_eq!(
            expression
                .evaluate(
                    "Sales.Status.Open",
                    &Variables::new(),
                    Some(&node),
                    &NoObjects
                )
                .unwrap(),
            FlowValue::String("Sales.Status.Open".into())
        );
        // Without a node, a bare identifier is unsupported.
        assert!(
            expression
                .evaluate("Name", &Variables::new(), None, &NoObjects)
                .is_err()
        );
        // With a node, an absent member is empty (nil), like Ruby's [].
        assert_eq!(
            expression
                .evaluate("Missing", &Variables::new(), Some(&node), &NoObjects)
                .unwrap(),
            FlowValue::Empty
        );
    }
}
