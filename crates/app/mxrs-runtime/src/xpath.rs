//! The XPath subset Mendix entity access rules are constrained with.
//!
//! Ports `lib/mxrb/runtime/access_control.rb`'s evaluator. It is deliberately
//! *not* the same machinery the flow interpreter uses for retrieve
//! constraints: that one rewrites `[group][group]` into a Mendix expression
//! and hands it to the expression engine, which is free to be permissive
//! because a wrong answer there returns the wrong rows. This one decides
//! whether a role may touch a record, so it is three-valued and fail-closed —
//! [`None`] means "this expression uses something the subset does not
//! understand", and a caller must read that as *no match*, never as a match.
//!
//! The grammar, in full: an optional `[...]` wrapper, `or`/`and` between
//! whitespace, `not(...)`, `true()`/`false()`, parenthesised groups, and
//! comparisons (`=`, `!=`, `>`, `<`, `>=`, `<=`) between a record member on
//! the left and a literal, `$variable`, or `[%CurrentUser%]` on the right.
//! Anything else is unsupported by construction.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::{DATETIME_MEMBER_PREFIX, SecurityContext};

/// Evaluates a constraint against one record.
///
/// `Some(true)` grants, `Some(false)` denies, and `None` means the expression
/// left this subset — which every caller must treat as a denial.
pub fn evaluate(
    expression: &str,
    record: &BTreeMap<String, Value>,
    context: &SecurityContext,
) -> Option<bool> {
    run(expression, Mode::Evaluate { record, context })
}

/// Whether the expression's *shape* is inside the subset, independent of any
/// record.
///
/// [`evaluate`] returns `None` both for an expression this evaluator cannot
/// read and for one it can read but whose member is absent from the record at
/// hand. Those mean different things to a person: the first is a constraint
/// mxrs will never honour and is worth reporting at boot, the second is an
/// ordinary per-row denial. This answers only the first question, by treating
/// every member and variable as resolvable.
pub fn is_supported(expression: &str) -> bool {
    run(expression, Mode::Shape).is_some()
}

/// Whether a list's constraint is one the runtime answers as Mendix would:
/// what [`is_supported`] reads, over the entity's own members — not an
/// association path (`Order_Customer/Customer/Name`), a page's variable
/// (`$currentObject`), or a token it does not resolve (`[%CurrentDateTime%]`,
/// `'[%CurrentObject%]'`). The current user, as a token or `$currentUser`,
/// is resolved.
pub fn is_list_supported(expression: &str) -> bool {
    if !is_supported(expression) {
        return false;
    }
    let characters: Vec<char> = expression.chars().collect();
    let mut quoted = false;
    for (index, character) in characters.iter().enumerate() {
        if *character == '\'' {
            quoted = !quoted;
            continue;
        }
        if *character == '[' && characters.get(index + 1) == Some(&'%') {
            let token: String = characters[index..].iter().take(15).collect();
            if !token.eq_ignore_ascii_case("[%CurrentUser%]") {
                return false;
            }
        }
        if quoted {
            continue;
        }
        if *character == '/' {
            return false;
        }
        if *character == '$' {
            let name: String = characters[index + 1..]
                .iter()
                .take_while(|character| character.is_alphanumeric() || **character == '_')
                .collect();
            if !name.eq_ignore_ascii_case("currentUser") {
                return false;
            }
        }
    }
    true
}

#[derive(Clone, Copy)]
enum Mode<'a> {
    Evaluate {
        record: &'a BTreeMap<String, Value>,
        context: &'a SecurityContext,
    },
    Shape,
}

fn run(expression: &str, mode: Mode<'_>) -> Option<bool> {
    let source = expression.trim();
    if source.is_empty() {
        return Some(true);
    }
    // A single surrounding predicate bracket is the authored spelling; the
    // evaluator works on its body. Only one is stripped, exactly as the
    // oracle does, so `[a][b]` stays unsupported rather than silently
    // becoming `a][b`.
    let source = match source
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    {
        Some(inner) => inner.trim(),
        None => source,
    };
    evaluate_boolean(source, mode)
}

fn evaluate_boolean(source: &str, mode: Mode<'_>) -> Option<bool> {
    let expression = strip_parentheses(source.trim());

    let parts = split_logical(expression, "or");
    if parts.len() > 1 {
        return combine(&parts, mode, Logical::Or);
    }
    let parts = split_logical(expression, "and");
    if parts.len() > 1 {
        return combine(&parts, mode, Logical::And);
    }

    if expression.eq_ignore_ascii_case("true()") {
        return Some(true);
    }
    if expression.eq_ignore_ascii_case("false()") {
        return Some(false);
    }
    if let Some(inner) = strip_not(expression) {
        return evaluate_boolean(inner, mode).map(|value| !value);
    }
    comparison(expression, mode)
}

#[derive(Clone, Copy)]
enum Logical {
    And,
    Or,
}

/// Any unsupported operand poisons the whole connective.
///
/// This is not Kleene logic, and it is not an oversight in the oracle: a
/// constraint the runtime only half understands describes a row set nobody
/// can predict, so `true or <unsupported>` denies rather than granting on the
/// half it did parse.
fn combine(parts: &[&str], mode: Mode<'_>, operator: Logical) -> Option<bool> {
    let mut values = Vec::with_capacity(parts.len());
    for part in parts {
        values.push(evaluate_boolean(part, mode)?);
    }
    Some(match operator {
        Logical::Or => values.iter().any(|value| *value),
        Logical::And => values.iter().all(|value| *value),
    })
}

fn comparison(expression: &str, mode: Mode<'_>) -> Option<bool> {
    let (raw_left, operator, raw_right) = split_comparison(expression)?;
    let left = operand(raw_left, mode, true);
    let right = operand(raw_right, mode, false);
    let (Some(left), Some(right)) = (left, right) else {
        return None;
    };
    match operator {
        Comparison::Equal => Some(left == right),
        Comparison::NotEqual => Some(left != right),
        ordering => {
            // Ruby's `left && right && left > right`: a nil or false operand
            // short-circuits before the comparison is attempted, and
            // comparing incompatible types raises, which the oracle rescues
            // to `false`.
            match (&left, &right) {
                (Operand::Unknown, _) | (_, Operand::Unknown) => Some(true),
                (Operand::Null, _) | (_, Operand::Null) => None,
                (Operand::Bool(false), _) => Some(false),
                (_, Operand::Bool(false)) => Some(false),
                _ => Some(
                    left.order(&right)
                        .is_some_and(|order| ordering.holds(order)),
                ),
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Comparison {
    Equal,
    NotEqual,
    Greater,
    Less,
    GreaterOrEqual,
    LessOrEqual,
}

impl Comparison {
    /// Longest spellings first: `>=` must not be read as `>` followed by a
    /// stray `=`, and `!=` must not be read as `=`.
    const SPELLINGS: [(&'static str, Comparison); 6] = [
        (">=", Comparison::GreaterOrEqual),
        ("<=", Comparison::LessOrEqual),
        ("!=", Comparison::NotEqual),
        ("=", Comparison::Equal),
        (">", Comparison::Greater),
        ("<", Comparison::Less),
    ];

    fn holds(self, order: std::cmp::Ordering) -> bool {
        use std::cmp::Ordering::{Equal, Greater, Less};
        match self {
            Comparison::Greater => order == Greater,
            Comparison::Less => order == Less,
            Comparison::GreaterOrEqual => matches!(order, Greater | Equal),
            Comparison::LessOrEqual => matches!(order, Less | Equal),
            Comparison::Equal => order == Equal,
            Comparison::NotEqual => order != Equal,
        }
    }
}

/// Finds the operator the way the oracle's non-greedy `(.+?)\s*(op)\s*(.+)`
/// does: the earliest operator that still leaves at least one character on
/// its left once the whitespace between them is discounted.
fn split_comparison(expression: &str) -> Option<(&str, Comparison, &str)> {
    let bytes = expression.as_bytes();
    for index in 1..bytes.len() {
        if !expression.is_char_boundary(index) {
            continue;
        }
        let Some((spelling, operator)) = Comparison::SPELLINGS
            .iter()
            .find(|(spelling, _)| expression[index..].starts_with(spelling))
            .copied()
        else {
            continue;
        };
        let left = expression[..index].trim_end();
        if left.is_empty() {
            continue;
        }
        let right = expression[index + spelling.len()..].trim_start();
        if right.is_empty() {
            return None;
        }
        return Some((left, operator, right));
    }
    None
}

#[derive(Debug, Clone, PartialEq)]
enum Operand {
    Null,
    Bool(bool),
    Number(f64),
    Text(String),
    /// A structured member value. It compares by equality and never orders,
    /// which is what the oracle's rescued `ArgumentError` amounts to.
    Structured(Value),
    /// A stand-in used only by [`is_supported`], where the question is
    /// whether the expression could ever be read, not what it answers. It
    /// compares and orders with anything so no operand is mistaken for an
    /// unsupported construct.
    Unknown,
}

impl Operand {
    fn order(&self, other: &Operand) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (Operand::Number(left), Operand::Number(right)) => left.partial_cmp(right),
            (Operand::Text(left), Operand::Text(right)) => Some(left.cmp(right)),
            (Operand::Bool(left), Operand::Bool(right)) => Some(left.cmp(right)),
            (Operand::Unknown, _) | (_, Operand::Unknown) => Some(std::cmp::Ordering::Equal),
            _ => None,
        }
    }
}

/// `None` is the oracle's `:unsupported`.
fn operand(source: &str, mode: Mode<'_>, record_side: bool) -> Option<Operand> {
    let token = source.trim();
    let quoted = token
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
        .filter(|_| token.len() >= 2);
    if let Some(text) = quoted {
        // `[System.owner = '[%CurrentUser%]']` is how Mendix itself spells
        // this constraint — the token is substituted inside the quotes. The
        // oracle checks for a quoted literal first and so never substitutes,
        // which leaves the single most common ownership rule comparing a user
        // id against the fourteen characters "[%CurrentUser%]" and denying
        // everyone. Reproducing that would be reproducing a defect, not
        // fidelity: mxrs already declines to copy native.rb's loop-body bug
        // for the same reason. The substitution is confined to a literal that
        // is *entirely* the token, so no ordinary string is affected.
        if text == "[%CurrentUser%]" {
            return match mode {
                Mode::Evaluate { context, .. } => user_identity(context),
                Mode::Shape => Some(Operand::Unknown),
            };
        }
        return Some(Operand::Text(text.replace("''", "'")));
    }
    if token.eq_ignore_ascii_case("true()") {
        return Some(Operand::Bool(true));
    }
    if token.eq_ignore_ascii_case("false()") {
        return Some(Operand::Bool(false));
    }
    if token.eq_ignore_ascii_case("empty") || token.eq_ignore_ascii_case("null") {
        return Some(Operand::Null);
    }
    if let Some(number) = parse_number(token) {
        return Some(Operand::Number(number));
    }
    let Mode::Evaluate { record, context } = mode else {
        // Shape mode: every member and variable is assumed resolvable, so
        // only a genuinely unreadable expression comes back unsupported.
        if token == "[%CurrentUser%]" || token.starts_with('$') || record_side {
            return Some(Operand::Unknown);
        }
        return None;
    };
    if token == "[%CurrentUser%]" {
        return user_identity(context);
    }
    if let Some(name) = token.strip_prefix('$') {
        if name.eq_ignore_ascii_case("currentuser") {
            return user_identity(context);
        }
        return context.variables.get(name).map(member_operand);
    }
    if record_side {
        return lookup(record, token).map(member_operand);
    }
    None
}

/// The caller's identity, or unsupported when there is none.
///
/// The oracle yields `nil` here, which then compares *equal* to an empty
/// member — so an unowned record would be granted to a caller who is not
/// anybody. Because this port substitutes the token in its canonical quoted
/// spelling (see [`operand`]), that path is reachable where it never was in
/// the oracle, and it would be a grant nobody asked for. A constraint that
/// names the current user is simply undecidable without one, so it denies.
fn user_identity(context: &SecurityContext) -> Option<Operand> {
    context.user.clone().map(Operand::Text)
}

/// Only the two literal shapes the oracle accepts: an optionally signed
/// integer, or an optionally signed decimal with digits on both sides. A
/// token like `1.` or `.5` is not a number here, so it falls through to the
/// member lookup and then to unsupported.
fn parse_number(token: &str) -> Option<f64> {
    let digits = token.strip_prefix('-').unwrap_or(token);
    if digits.is_empty() {
        return None;
    }
    let (whole, fraction) = match digits.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (digits, None),
    };
    if whole.is_empty() || !whole.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if let Some(fraction) = fraction
        && (fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    token.parse().ok()
}

/// A stored datetime carries [`DATETIME_MEMBER_PREFIX`]; it becomes the
/// instant it represents rather than the tagged string, so a constraint never
/// compares against storage bookkeeping. Comparing an instant to a quoted date
/// literal then yields no ordering — the same outcome the oracle reaches by
/// rescuing `Time <=> String`.
fn member_operand(value: &Value) -> Operand {
    match value {
        Value::Null => Operand::Null,
        Value::Bool(value) => Operand::Bool(*value),
        Value::Number(number) => number.as_f64().map_or(Operand::Null, Operand::Number),
        Value::String(text) => match text
            .strip_prefix(DATETIME_MEMBER_PREFIX)
            .and_then(|seconds| seconds.parse::<f64>().ok())
        {
            Some(seconds) => Operand::Number(seconds),
            None => Operand::Text(text.clone()),
        },
        other => Operand::Structured(other.clone()),
    }
}

/// The member path as written, then its last segment. `System.owner` resolves
/// against a record that stores plain `owner`, which is how authored
/// constraints usually spell system members.
fn lookup<'a>(record: &'a BTreeMap<String, Value>, path: &str) -> Option<&'a Value> {
    if let Some(value) = record.get(path) {
        return Some(value);
    }
    let tail = path.rsplit(['/', '.']).next()?;
    if tail == path {
        return None;
    }
    record.get(tail)
}

/// Splits on a whitespace-delimited keyword, case-insensitively, the way the
/// oracle's `/\s+or\s+/i` does. Quoting is not respected — a literal
/// containing ` and ` splits too, which is the oracle's behaviour and only
/// ever produces an unsupported fragment, never a wrong grant.
fn split_logical<'a>(source: &'a str, keyword: &str) -> Vec<&'a str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let bytes = source.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if !bytes[index].is_ascii_whitespace() {
            index += 1;
            continue;
        }
        let space_start = index;
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let word_start = index;
        if source[word_start..].len() < keyword.len()
            || !source[word_start..word_start + keyword.len()].eq_ignore_ascii_case(keyword)
        {
            continue;
        }
        let word_end = word_start + keyword.len();
        if word_end >= bytes.len() || !bytes[word_end].is_ascii_whitespace() {
            continue;
        }
        let mut after = word_end;
        while after < bytes.len() && bytes[after].is_ascii_whitespace() {
            after += 1;
        }
        parts.push(&source[start..space_start]);
        start = after;
        index = after;
    }
    parts.push(&source[start..]);
    parts
}

/// Repeatedly removes a leading `(` together with a trailing `)`.
///
/// This is the oracle's `strip_parentheses`, reproduced including its
/// limitation: it does not check that the two are actually paired, so
/// `(a = 1) and (b = 2)` is mangled into `a = 1) and (b = 2` and ends up
/// unsupported. Matching brackets here would make mxrs *grant* on
/// constraints mxrb denies, and being more permissive than the oracle is the
/// one direction a security evaluator must never diverge in. Widening it is a
/// deliberate decision for later, not a cleanup.
fn strip_parentheses(source: &str) -> &str {
    let mut source = source.trim();
    while source.starts_with('(') && source.ends_with(')') && source.len() >= 2 {
        source = source[1..source.len() - 1].trim();
    }
    source
}

/// `not(...)` spanning the whole expression, with the oracle's greedy body.
fn strip_not(expression: &str) -> Option<&str> {
    let rest = if expression.len() >= 3 && expression[..3].eq_ignore_ascii_case("not") {
        &expression[3..]
    } else {
        return None;
    };
    let rest = rest.trim_start();
    rest.strip_prefix('(')?.strip_suffix(')')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.clone()))
            .collect()
    }

    fn context(user: Option<&str>, variables: &[(&str, Value)]) -> SecurityContext {
        SecurityContext {
            user: user.map(ToString::to_string),
            variables: variables
                .iter()
                .map(|(name, value)| ((*name).to_string(), value.clone()))
                .collect(),
            ..SecurityContext::default()
        }
    }

    #[test]
    fn the_supported_subset_evaluates_against_a_record() {
        let order = record(&[
            ("Number", Value::from(7)),
            ("Name", Value::String("blue".into())),
            ("Open", Value::Bool(true)),
            ("owner", Value::String("alice".into())),
            ("Note", Value::Null),
        ]);
        let context = context(Some("alice"), &[("wanted", Value::String("blue".into()))]);
        let yes = |source: &str| {
            assert_eq!(
                evaluate(source, &order, &context),
                Some(true),
                "expected {source} to hold"
            );
        };
        let no = |source: &str| {
            assert_eq!(
                evaluate(source, &order, &context),
                Some(false),
                "expected {source} not to hold"
            );
        };

        // An empty constraint is an unconstrained rule, not a denial.
        yes("");
        yes("   ");
        yes("[Number = 7]");
        yes("Number = 7");
        no("[Number = 8]");
        yes("[Number != 8]");
        yes("[Number > 6]");
        yes("[Number >= 7]");
        yes("[Number < 8]");
        yes("[Number <= 7]");
        no("[Number > 7]");
        yes("[Name = 'blue']");
        no("[Name = 'red']");
        yes("[Open = true()]");
        yes("[Note = empty]");
        no("[Name = empty]");
        yes("[System.owner = '[%CurrentUser%]']");
        yes("[owner = $currentuser]");
        yes("[Name = $wanted]");
        yes("[true()]");
        no("[false()]");
        yes("[not(Number = 8)]");
        no("[not(Number = 7)]");
        yes("[Number = 7 and Name = 'blue']");
        no("[Number = 8 and Name = 'blue']");
        yes("[Number = 8 or Name = 'blue']");
        no("[Number = 8 or Name = 'red']");
        yes("[Number = 7 AND Name = 'blue']");
        yes("[((Number = 7))]");
        // Quote escaping, and a decimal literal.
        let quoted = record(&[("Name", Value::String("o'clock".into()))]);
        assert_eq!(
            evaluate("[Name = 'o''clock']", &quoted, &context),
            Some(true)
        );
        let ratio = record(&[("Ratio", Value::from(0.25))]);
        assert_eq!(evaluate("[Ratio = 0.25]", &ratio, &context), Some(true));
        assert_eq!(evaluate("[Ratio < 0.5]", &ratio, &context), Some(true));
    }

    /// Everything outside the subset must read as a denial. Each of these
    /// would be a grant if the evaluator guessed.
    #[test]
    fn anything_outside_the_subset_is_unsupported_rather_than_a_match() {
        let order = record(&[("Number", Value::from(7))]);
        let context = context(Some("alice"), &[]);
        for source in [
            // An unknown member: the rule cannot be about this record.
            "[Missing = 1]",
            // An unknown variable.
            "[Number = $absent]",
            // A function this subset does not implement.
            "[contains(Name, 'x')]",
            // An association step.
            "[Order_Customer/Customer/Name = 'a']",
            // Not a comparison at all.
            "[Number]",
            // A member on the right-hand side is not resolved.
            "[7 = Number]",
            // Two predicates: only one bracket pair is stripped.
            "[Number = 7][Number = 7]",
            // An unsupported branch poisons the connective even when the
            // other side is decidable.
            "[Number = 7 or Missing = 1]",
            "[Number = 7 and Missing = 1]",
        ] {
            assert_eq!(evaluate(source, &order, &context), None, "{source}");
        }
    }

    /// Pinned because it is a real limitation of the oracle's parenthesis
    /// stripping, and because "fix" here means granting where mxrb denies.
    #[test]
    fn grouped_operands_around_a_connective_stay_unsupported_like_the_oracle() {
        let order = record(&[
            ("Number", Value::from(7)),
            ("Name", Value::String("b".into())),
        ]);
        let context = context(None, &[]);
        assert_eq!(
            evaluate("[(Number = 7) and (Name = 'b')]", &order, &context),
            None
        );
        // The same constraint without the inner groups is fully supported.
        assert_eq!(
            evaluate("[Number = 7 and Name = 'b']", &order, &context),
            Some(true)
        );
    }

    /// The canonical Mendix ownership rule, and the two divergences from the
    /// oracle it depends on: the token is substituted inside its quotes, and
    /// a caller with no identity cannot satisfy it at all.
    #[test]
    fn the_current_user_token_is_substituted_and_denies_without_a_caller() {
        let owned = record(&[("owner", Value::String("alice".into()))]);
        let unowned = record(&[("owner", Value::Null)]);
        let alice = context(Some("alice"), &[]);
        let bob = context(Some("bob"), &[]);
        let anonymous = context(None, &[]);

        for spelling in [
            "[System.owner = '[%CurrentUser%]']",
            "[owner = [%CurrentUser%]]",
            "[owner = $currentuser]",
        ] {
            assert_eq!(evaluate(spelling, &owned, &alice), Some(true), "{spelling}");
            assert_eq!(evaluate(spelling, &owned, &bob), Some(false), "{spelling}");
            // No caller, no decision — never a grant, and never a match on
            // an unowned record either.
            assert_eq!(evaluate(spelling, &owned, &anonymous), None, "{spelling}");
            assert_eq!(evaluate(spelling, &unowned, &anonymous), None, "{spelling}");
        }

        // The substitution is confined to a literal that is exactly the
        // token; ordinary text containing it is still ordinary text.
        let labelled = record(&[("owner", Value::String("see [%CurrentUser%] below".into()))]);
        assert_eq!(
            evaluate("[owner = 'see [%CurrentUser%] below']", &labelled, &alice),
            Some(true)
        );
    }

    /// Ordering across incompatible types is the oracle's rescued
    /// `ArgumentError`, and an ordering against `empty` short-circuits to
    /// unsupported rather than to a match.
    #[test]
    fn incomparable_operands_do_not_order() {
        let order = record(&[
            ("Number", Value::from(7)),
            ("Name", Value::String("blue".into())),
            ("Note", Value::Null),
        ]);
        let context = context(None, &[]);
        assert_eq!(evaluate("[Number > 'blue']", &order, &context), Some(false));
        assert_eq!(evaluate("[Name > 3]", &order, &context), Some(false));
        assert_eq!(evaluate("[Note > 3]", &order, &context), None);
        assert_eq!(evaluate("[Note = empty]", &order, &context), Some(true));
    }

    #[test]
    fn a_stored_datetime_compares_as_an_instant_not_as_its_tag() {
        let order = record(&[(
            "When",
            Value::String(format!("{DATETIME_MEMBER_PREFIX}1758462896")),
        )]);
        let context = context(None, &[]);
        assert_eq!(
            evaluate("[When > 1000000000]", &order, &context),
            Some(true)
        );
        assert_eq!(
            evaluate("[When < 1000000000]", &order, &context),
            Some(false)
        );
        // Against a quoted date it yields no ordering, like Time <=> String.
        assert_eq!(
            evaluate("[When > '2020-01-01']", &order, &context),
            Some(false)
        );
    }
}
