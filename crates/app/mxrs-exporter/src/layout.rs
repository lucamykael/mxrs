//! How generated Rust is laid out beyond what rustfmt decides.
//!
//! Two things rustfmt does not do for a flow body:
//!
//! - A string literal too long for its line makes rustfmt give up on the
//!   whole statement holding it — in a loop body, every activity of the
//!   loop. Such a literal, and any text that spans lines (an XPath written
//!   over several), is named at the top of the file instead, where it reads
//!   as what it is: `const BUILDING_XPATH: &str = "...";`.
//! - Statements are spaced by what they are: declarations (the flow's
//!   parameters and return type, one-line `let`s) are stacked; every other
//!   operation has a blank line before and after it.

use std::collections::HashSet;

/// The widest line rustfmt lays a statement out in.
const MAX_WIDTH: usize = 100;

/// What a line of a flow body is indented by inside its function.
#[cfg(test)]
const BODY_INDENT: usize = 4;

/// Text longer than this many lines, or characters, is data rather than
/// code: it is kept in a file of its own beside the source and included.
const DATA_LINES: usize = 60;
const DATA_CHARACTERS: usize = 4000;

/// What naming a function body's long literals produced.
#[derive(Debug, Default)]
pub(crate) struct Hoisted {
    /// The rewritten source.
    pub(crate) source: String,
    /// Each constant's declaration, in order of use.
    pub(crate) constants: Vec<String>,
    /// `(path, contents)` of each text kept beside the source, relative
    /// to its `data/` folder; its constant includes it from there.
    pub(crate) data: Vec<(String, String)>,
}

/// Replaces the string literals of the function bodies in `source` that
/// would not fit their line, or that span lines, by constants.
///
/// Text that is data is kept in `data/<data_folder>/`, one folder per
/// source file: two files of one module never share one.
pub(crate) fn hoist_literals_in(source: &str, data_folder: &str) -> Hoisted {
    let lines: Vec<String> = source.lines().map(str::to_string).collect();
    let mut out = Vec::with_capacity(lines.len());
    let mut hoisted = Hoisted::default();
    let mut taken = HashSet::new();
    let mut index = 0;
    while index < lines.len() {
        let line = &lines[index];
        out.push(line.clone());
        index += 1;
        if !opens_function(line) {
            continue;
        }
        let closing = " ".repeat(indent_of(line)) + "}";
        let Some(length) = lines[index..].iter().position(|line| *line == closing) else {
            continue;
        };
        out.extend(hoist(
            &lines[index..index + length],
            0,
            data_folder,
            &mut taken,
            &mut hoisted,
        ));
        index += length;
    }
    hoisted.source = out.join("\n");
    if source.ends_with('\n') {
        hoisted.source.push('\n');
    }
    hoisted
}

/// As [`hoist_literals_in`], for `lines` of a function body written without
/// the body's own indentation.
#[cfg(test)]
pub(crate) fn hoist_literals(lines: &[String]) -> (Vec<String>, Hoisted) {
    let mut hoisted = Hoisted::default();
    let lines = hoist(
        lines,
        BODY_INDENT,
        "flow",
        &mut HashSet::new(),
        &mut hoisted,
    );
    (lines, hoisted)
}

/// `lines` with their long literals named, each line indented by `base`
/// more than it is written.
fn hoist(
    lines: &[String],
    base: usize,
    data_folder: &str,
    taken: &mut HashSet<String>,
    hoisted: &mut Hoisted,
) -> Vec<String> {
    let mut out = Vec::with_capacity(lines.len());
    for (index, line) in lines.iter().enumerate() {
        let indent = line.len() - line.trim_start().len() + base;
        let mut rewritten = String::new();
        let mut rest = line.as_str();
        while let Some((before, literal, after)) = next_literal(rest) {
            rewritten.push_str(before);
            let value = decode(literal);
            let multiline = value.trim_end_matches('\n').contains('\n');
            // rustfmt puts a literal that does not fit on a line of its own,
            // inside the call that takes it: `mx(` and a trailing `),`.
            let too_long = indent + 4 + literal.len() + "mx(),".len() > MAX_WIDTH;
            if multiline || too_long {
                let prefix = format!("{rewritten}{before}");
                let name = unique(constant_name(lines, index, &prefix), taken);
                if value.lines().count() > DATA_LINES || value.len() > DATA_CHARACTERS {
                    let file = format!("{data_folder}/{}.txt", name.to_ascii_lowercase());
                    hoisted.constants.push(format!(
                        "const {name}: &str = include_str!(\"data/{file}\");"
                    ));
                    hoisted.data.push((file, value));
                } else {
                    hoisted.constants.push(format!(
                        "const {name}: &str = {};",
                        multiline_literal(&value)
                    ));
                }
                rewritten.push_str(&name);
            } else {
                rewritten.push_str(literal);
            }
            rest = after;
        }
        rewritten.push_str(rest);
        out.push(rewritten);
    }
    out
}

/// The first string literal of `text`: what precedes it, the literal with
/// its quotes, and what follows.
fn next_literal(text: &str) -> Option<(&str, &str, &str)> {
    let start = text.find('"')?;
    let bytes = text.as_bytes();
    let mut end = start + 1;
    while end < bytes.len() {
        match bytes[end] {
            b'\\' => end += 2,
            b'"' => return Some((&text[..start], &text[start..=end], &text[end + 1..])),
            _ => end += 1,
        }
    }
    None
}

/// The value of a literal written the way `{:?}` writes a string.
fn decode(literal: &str) -> String {
    let inner = &literal[1..literal.len() - 1];
    let mut value = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            value.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => value.push('\n'),
            Some('t') => value.push('\t'),
            Some('r') => value.push('\r'),
            Some('0') => value.push('\0'),
            Some('u') => {
                let code: String = chars
                    .by_ref()
                    .skip_while(|c| *c == '{')
                    .take_while(|c| *c != '}')
                    .collect();
                if let Some(c) = u32::from_str_radix(&code, 16).ok().and_then(char::from_u32) {
                    value.push(c);
                }
            }
            Some(other) => value.push(other),
            None => {}
        }
    }
    value
}

/// A literal of `value` that breaks its lines where the value does.
fn multiline_literal(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '\n' => out.push('\n'),
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\'' => out.push('\''),
            c => out.extend(c.escape_debug()),
        }
    }
    out.push('"');
    out
}

fn unique(name: String, taken: &mut HashSet<String>) -> String {
    let mut candidate = name.clone();
    let mut counter = 2;
    while !taken.insert(candidate.clone()) {
        candidate = format!("{name}_{counter}");
        counter += 1;
    }
    candidate
}

/// A name for the literal at `before` on line `index`, from what the
/// statement holding it does: the XPath of the list a retrieve fills, the
/// member a change sets, the value a flow returns.
fn constant_name(lines: &[String], index: usize, before: &str) -> String {
    let line = &lines[index];
    if let Some(name) = macro_constant_name(line, before) {
        return name;
    }
    let subject = subject(lines, index);
    let joined = |parts: &[&str]| {
        parts
            .iter()
            .filter(|part| !part.is_empty())
            .map(|part| crate::snake_ident(part).to_ascii_uppercase())
            .collect::<Vec<_>>()
            .join("_")
    };
    if before.contains(".xpath(") {
        return joined(&[&subject, "xpath"]);
    }
    if let Some(member) = set_member(before) {
        return joined(&[&subject, &member]);
    }
    if before.contains("return_with(") {
        return "RETURN_VALUE".to_string();
    }
    if let Some(argument) =
        quoted_after(before, ".argument(").or_else(|| quoted_after(before, ".parameter_of("))
    {
        return joined(&[&argument, "argument"]);
    }
    // A literal that is an argument on a line of its own is named by the
    // call it is passed to.
    if before.trim().is_empty() && enclosing_call(lines, index).as_deref() == Some("flow.log") {
        return joined(&[&subject, "log_message"]);
    }
    if before.contains("flow.log(") && !before.contains("|log|") {
        return joined(&[&subject, "log_message"]);
    }
    if let Some(call) = last_method_call(before) {
        let name = joined(&[&subject, &call]);
        return if name.is_empty() {
            "VALUE".to_string()
        } else {
            name
        };
    }
    let previous = index.checked_sub(1).map(|index| lines[index].trim());
    if before.contains("decision(")
        || before.contains("switch(")
        || before.contains("while_loop(")
        || previous.is_some_and(|line| line.ends_with("decision(") || line.ends_with("switch("))
        || line.contains("filter_by(")
        || line.contains("find_by(")
    {
        return joined(&[&subject, "condition"]);
    }
    joined(&[&subject, "expression"])
}

/// A name for a literal an activity macro takes, from what the macro
/// declares (its `name = "..."`, or the name Studio Pro would give) and what
/// the literal is to it: `ORDER_LIST_XPATH`, `NEW_ORDER_NUMBER`,
/// `PAID_ORDERS_CONDITION`.
fn macro_constant_name(line: &str, before: &str) -> Option<String> {
    let call = line
        .trim_start()
        .strip_prefix("let ")
        .and_then(|rest| rest.split_once(" = "))
        .map_or(line.trim_start(), |(_, call)| call);
    let (macro_name, arguments) = call.split_once("!(")?;
    if !macro_name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c == '_')
    {
        return None;
    }
    let arguments = arguments.strip_prefix("flow, ").unwrap_or(arguments);
    let named = line
        .split_once("name = \"")
        .and_then(|(_, rest)| rest.split('"').next())
        .map(str::to_string);
    let first: String = arguments
        .trim_start_matches('&')
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    let subject = named.unwrap_or_else(|| match macro_name {
        "create_object" => format!("New{first}"),
        "retrieve" if line.contains(", first") => first.clone(),
        "retrieve" | "create_list" => format!("{first}List"),
        "log" => "log".to_string(),
        _ => first.strip_prefix("value_").unwrap_or(&first).to_string(),
    });
    let tail = before.trim_end();
    let role = if tail.ends_with("xpath =") {
        "xpath".to_string()
    } else if tail.ends_with("find_by =") || tail.ends_with("filter_by =") {
        "condition".to_string()
    } else if let Some(key) = tail
        .strip_suffix(':')
        .or_else(|| tail.strip_suffix('='))
        .map(|rest| {
            rest.trim_end()
                .rsplit(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .filter(|key| !key.is_empty())
    {
        key
    } else if macro_name == "log" {
        "message".to_string()
    } else {
        String::new()
    };
    let name = [subject.as_str(), role.as_str()]
        .iter()
        .filter(|part| !part.is_empty())
        .map(|part| crate::snake_ident(part).to_ascii_uppercase())
        .collect::<Vec<_>>()
        .join("_");
    (!name.is_empty()).then_some(name)
}

/// The variable the statement around line `index` makes or changes: the
/// name a `flow.retrieve("...")` or `flow.create("...")` gives, the binding
/// a `flow.change(&...)` changes, or the binding a `let` holds.
fn subject(lines: &[String], index: usize) -> String {
    let indent = |line: &str| line.len() - line.trim_start().len();
    let mut level = indent(&lines[index]) + 1;
    for line in lines[..=index].iter().rev() {
        let current = indent(line);
        if current >= level {
            continue;
        }
        level = current;
        let trimmed = line.trim_start();
        let call = trimmed
            .find("flow.")
            .map(|start| &trimmed[start + "flow.".len()..]);
        if let Some(name) = call.and_then(|call| {
            let open = call.find('(')?;
            let arguments = &call[open + 1..];
            if let Some(name) = arguments.strip_prefix('"') {
                return name.split('"').next().map(str::to_string);
            }
            arguments
                .strip_prefix('&')
                .map(|binding| binding.split([',', ')']).next().unwrap_or_default())
                .map(|binding| {
                    binding
                        .strip_prefix("value_")
                        .unwrap_or(binding)
                        .to_string()
                })
        }) {
            return name;
        }
        if let Some(binding) = trimmed.strip_prefix("let ") {
            let binding = binding.split([' ', ':']).next().unwrap_or_default();
            return binding
                .strip_prefix("value_")
                .unwrap_or(binding)
                .to_string();
        }
        if current == 0 {
            break;
        }
    }
    String::new()
}

/// What the innermost method call open in `before` passes the literal
/// to: its first quoted argument when it has one (`.with("Value", ` →
/// `Value`), else the receiver and method (`log.parameter(` →
/// `log_parameter`). Calls on the flow itself say only their method.
fn last_method_call(before: &str) -> Option<String> {
    let identifier = |c: char| c.is_ascii_alphanumeric() || c == '_';
    for (open, _) in before
        .match_indices('(')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        let head = &before[..open];
        let method_start = head.trim_end_matches(identifier).len();
        let method = &head[method_start..];
        let Some(receiver_end) = head[..method_start].strip_suffix('.') else {
            continue;
        };
        if method.is_empty() || method == "set" || method == "new" {
            continue;
        }
        let receiver = &receiver_end[receiver_end.trim_end_matches(identifier).len()..];
        // What a flow declares or changes is named by the variable alone.
        if receiver == "flow" && matches!(method, "create_variable" | "change_variable") {
            return Some(String::new());
        }
        if matches!(method, "argument" | "entity_argument" | "with")
            && let Some(argument) = quoted_after(&before[open..], "(")
        {
            return Some(argument);
        }
        return Some(
            if receiver.is_empty() || receiver == "flow" || receiver == "call" {
                method.to_string()
            } else {
                format!("{receiver}_{method}")
            },
        );
    }
    None
}

/// The call whose argument list holds line `index`: `flow.log` for a line
/// inside `flow.log(\n ...`.
fn enclosing_call(lines: &[String], index: usize) -> Option<String> {
    let indent = |line: &str| line.len() - line.trim_start().len();
    let level = indent(&lines[index]);
    let opener = lines[..index]
        .iter()
        .rev()
        .find(|line| indent(line) < level)?;
    let call = opener.trim().strip_suffix('(')?;
    let call = call.rsplit([' ', '(']).next()?;
    Some(call.to_string())
}

/// The member a `.set(Entity::member(), ...)` or a `.set(MemberName::...("A.B.C"), ...)`
/// sets.
fn set_member(before: &str) -> Option<String> {
    let call = &before[before.rfind(".set(")? + ".set(".len()..];
    if let Some(name) = quoted_after(call, "MemberName::attribute(")
        .or_else(|| quoted_after(call, "MemberName::association("))
    {
        return name.rsplit('.').next().map(str::to_string);
    }
    let path = call.split('(').next()?;
    path.rsplit("::").next().map(str::to_string)
}

fn quoted_after(text: &str, prefix: &str) -> Option<String> {
    let start = text.find(prefix)? + prefix.len();
    let rest = text[start..].strip_prefix('"')?;
    rest.split('"').next().map(str::to_string)
}

/// `field: field` in a struct literal written the way Rust writes it, the
/// field alone: `create_object!(flow, Order { customer })`.
pub(crate) fn shorthand_fields(source: &str) -> String {
    let bytes = source.as_bytes();
    let identifier = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out = String::with_capacity(source.len());
    let mut index = 0;
    while index < bytes.len() {
        let c = bytes[index];
        if c == b'"' {
            // A string literal is copied as it is.
            let start = index;
            index += 1;
            while index < bytes.len() && bytes[index] != b'"' {
                index += if bytes[index] == b'\\' { 2 } else { 1 };
            }
            index = (index + 1).min(bytes.len());
            out.push_str(&source[start..index]);
            continue;
        }
        let starts_word =
            (c.is_ascii_alphabetic() || c == b'_') && (index == 0 || !identifier(bytes[index - 1]));
        if starts_word {
            let start = index;
            while index < bytes.len() && identifier(bytes[index]) {
                index += 1;
            }
            let word = &source[start..index];
            let opens = source[..start].trim_end().ends_with(['{', ',']);
            let rest = &source[index..];
            let value = rest
                .strip_prefix(": ")
                .and_then(|rest| rest.strip_prefix(word));
            let closes = value.is_some_and(|after| {
                !after.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
                    && after.trim_start().starts_with([',', '}'])
            });
            out.push_str(word);
            if opens && closes {
                index += ": ".len() + word.len();
            }
            continue;
        }
        out.push(c as char);
        index += 1;
    }
    out
}

/// Spaces the statements of every flow body in `source` (formatted by
/// rustfmt): declarations stacked, every other operation set apart by a
/// blank line. Bodies holding a literal that spans lines are left as they
/// are, since their lines are not all code.
pub(crate) fn space_statements(source: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let mut out = Vec::with_capacity(lines.len());
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        out.push(line.to_string());
        index += 1;
        // Only a flow's body is a sequence of activities; any other
        // function is laid out as rustfmt leaves it.
        if !opens_function(line) || !line.contains("FlowBuilder") {
            continue;
        }
        let indent = indent_of(line);
        let closing = " ".repeat(indent) + "}";
        let Some(length) = lines[index..].iter().position(|line| *line == closing) else {
            continue;
        };
        let body = &lines[index..index + length];
        if body.iter().any(|line| opens_string(line)) {
            out.extend(body.iter().map(|line| line.to_string()));
        } else {
            out.extend(space_block(body, indent + 4));
        }
        index += length;
    }
    let mut spaced = out.join("\n");
    if source.ends_with('\n') {
        spaced.push('\n');
    }
    spaced
}

/// Whether a string literal starts on `line` and goes on past its end:
/// the line's unescaped quotes are odd in number.
fn opens_string(line: &str) -> bool {
    let mut open = false;
    let mut escaped = false;
    for c in line.chars() {
        match c {
            '\\' if open && !escaped => escaped = true,
            '"' if !escaped => open = !open,
            _ => escaped = false,
        }
        if c != '\\' {
            escaped = false;
        }
    }
    open
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// The line that opens a function's body.
fn opens_function(line: &str) -> bool {
    let trimmed = line.trim_start();
    let signature = [
        "pub fn ",
        "fn ",
        "pub(crate) fn ",
        "pub async fn ",
        "async fn ",
    ]
    .iter()
    .any(|prefix| trimmed.starts_with(prefix))
        || trimmed.starts_with(") -> ")
        || trimmed == ") {";
    signature && trimmed.ends_with('{') && !trimmed.ends_with("{}")
}

/// The statements of one block whose statements are indented by `indent`.
fn space_block(lines: &[&str], indent: usize) -> Vec<String> {
    space_block_as(lines, indent, true)
}

/// The statements of one block: spaced when they are a flow's or a
/// function's own statements, stacked when they are one activity's options
/// (`|retrieve| { ... }`).
fn space_block_as(lines: &[&str], indent: usize, statements_of_flow: bool) -> Vec<String> {
    let mut statements: Vec<Vec<&str>> = Vec::new();
    let mut pending_comment: Vec<&str> = Vec::new();
    for line in lines.iter().copied().filter(|line| !line.trim().is_empty()) {
        let at_level = indent_of(line) == indent;
        let trimmed = line.trim_start();
        let continues = !at_level || trimmed.starts_with(['}', ')', ']', '.']);
        if trimmed.starts_with("//") && at_level {
            pending_comment.push(line);
        } else if continues && let Some(current) = statements.last_mut() {
            current.push(line);
        } else {
            let mut statement = std::mem::take(&mut pending_comment);
            statement.push(line);
            statements.push(statement);
        }
    }
    if let Some(current) = statements.last_mut() {
        current.append(&mut pending_comment);
    }
    // A block of items that end in commas is a list — arguments, fields,
    // match arms — not of statements: nothing is spaced in it.
    let is_list = statements
        .iter()
        .any(|statement| statement.last().is_some_and(|line| line.ends_with(',')));
    let mut out = Vec::new();
    let mut previous_declares = None;
    for statement in &statements {
        let declares = declares(statement);
        if statements_of_flow
            && !is_list
            && previous_declares.is_some_and(|previous| !(previous && declares))
        {
            out.push(String::new());
        }
        out.extend(space_nested(statement));
        previous_declares = Some(declares);
    }
    out
}

/// A statement that declares rather than does: a flow's parameter or
/// return type, or a `let` on one line.
fn declares(statement: &[&str]) -> bool {
    let Some(first) = statement
        .iter()
        .find(|line| !line.trim_start().starts_with("//"))
    else {
        return false;
    };
    let first = first.trim_start();
    let call = first
        .strip_prefix("let ")
        .and_then(|rest| rest.split_once(" = "))
        .map_or(first, |(_, call)| call);
    call.starts_with("flow.parameter")
        || call.starts_with("flow.returns(")
        || (first.starts_with("let ")
            && statement
                .iter()
                .filter(|line| !line.trim_start().starts_with("//"))
                .count()
                == 1)
}

/// `statement` with each block it opens spaced in turn.
fn space_nested(statement: &[&str]) -> Vec<String> {
    let mut out = Vec::with_capacity(statement.len());
    let mut index = 0;
    while index < statement.len() {
        let line = statement[index];
        out.push(line.to_string());
        index += 1;
        let trimmed = line.trim_end();
        if !trimmed.ends_with('{') {
            continue;
        }
        let indent = indent_of(line);
        let Some(length) = statement[index..]
            .iter()
            .position(|line| indent_of(line) == indent && line.trim_start().starts_with('}'))
        else {
            continue;
        };
        // A closure over the flow holds the flow's statements; any other
        // block is one activity's options.
        let of_flow = trimmed.contains("|flow|") || trimmed.contains("|flow, ");
        out.extend(space_block_as(
            &statement[index..index + length],
            indent + 4,
            of_flow,
        ));
        index += length;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    #[test]
    fn a_literal_too_long_or_over_several_lines_is_named_at_the_top() {
        let body = lines(concat!(
            "let building = flow.retrieve(\"Building\", Ref::<Buildings>::new(), |retrieve| {\n",
            "    retrieve.xpath(\"[\\n  (\\n    BuildingID = $buildingID\\n  )\\n]\");\n",
            "});\n",
            "flow.create(\"NewParameters\", Ref::<Parameters>::new(), |create| {\n",
            "    create.set(Parameters::measurement_unit(), mx(\"$IteratorSPCProgramParameter/SPCProgram.SPCProgramParameter_MeasurementUnit/Catalogs.MeasurementUnit/MeasurementUnitName\"));\n",
            "    create.set(Parameters::name(), mx(\"$Short\"));\n",
            "});\n",
            "flow.return_with(mx(\"$NewRoot\\n\"));",
        ));
        let (body, hoisted) = hoist_literals(&body);
        assert_eq!(
            hoisted.constants,
            [
                "const BUILDING_XPATH: &str = \"[\n  (\n    BuildingID = $buildingID\n  )\n]\";",
                "const NEW_PARAMETERS_MEASUREMENT_UNIT: &str = \"$IteratorSPCProgramParameter/SPCProgram.SPCProgramParameter_MeasurementUnit/Catalogs.MeasurementUnit/MeasurementUnitName\";",
            ]
        );
        assert_eq!(body[1], "    retrieve.xpath(BUILDING_XPATH);");
        assert_eq!(
            body[4],
            "    create.set(Parameters::measurement_unit(), mx(NEW_PARAMETERS_MEASUREMENT_UNIT));"
        );
        // A trailing line break alone does not make text span lines.
        assert_eq!(body[7], "flow.return_with(mx(\"$NewRoot\\n\"));");
    }

    #[test]
    fn text_that_is_data_is_kept_beside_the_source() {
        let payload = (0..100)
            .map(|n| format!("{{\"n\": {n}}}"))
            .collect::<Vec<_>>()
            .join("\n");
        let body = vec![format!(
            "flow.create_variable(\"Payload\", DataType::String, mx({payload:?}));"
        )];
        let (body, hoisted) = hoist_literals(&body);
        assert_eq!(
            body[0],
            "flow.create_variable(\"Payload\", DataType::String, mx(PAYLOAD));"
        );
        assert_eq!(
            hoisted.constants,
            ["const PAYLOAD: &str = include_str!(\"data/flow/payload.txt\");"]
        );
        assert_eq!(hoisted.data, [("flow/payload.txt".to_string(), payload)]);
    }

    #[test]
    fn a_field_set_to_its_namesake_is_written_alone() {
        assert_eq!(
            shorthand_fields(
                "create_object!(flow, Order { customer: customer, total: total_x, note: \"a: a\" })"
            ),
            "create_object!(flow, Order { customer, total: total_x, note: \"a: a\" })"
        );
        assert_eq!(
            shorthand_fields("Order {\n    customer: customer\n}"),
            "Order {\n    customer\n}"
        );
    }

    #[test]
    fn declarations_are_stacked_and_operations_set_apart() {
        let source = concat!(
            "#[microflow(SUB)]\n",
            "pub fn check(flow: &mut FlowBuilder) {\n",
            "    flow.parameter_of(\n",
            "        \"Plan\",\n",
            "        DataType::object::<Plan>(),\n",
            "        |parameter| {\n",
            "            parameter.required(true);\n",
            "        },\n",
            "    );\n",
            "    flow.returns(DataType::Boolean);\n",
            "    let first = flow.create_variable(\"First\", DataType::Boolean, mx(\"true\"));\n",
            "    let second = flow.create_variable(\"Second\", DataType::Boolean, mx(\"true\"));\n",
            "    flow.decision(\n",
            "        mx(\"$First\"),\n",
            "        |flow| {\n",
            "            flow.commit(&first);\n",
            "            flow.commit(&second);\n",
            "        },\n",
            "        |_| {},\n",
            "    );\n",
            "    let list = flow.retrieve(\"List\", Ref::<Plan>::new(), |retrieve| {\n",
            "        retrieve.xpath(\"[Open]\");\n",
            "        retrieve.first();\n",
            "    });\n",
            "    flow.return_with(mx(\"true\"));\n",
            "}\n",
        );
        let expected = concat!(
            "#[microflow(SUB)]\n",
            "pub fn check(flow: &mut FlowBuilder) {\n",
            "    flow.parameter_of(\n",
            "        \"Plan\",\n",
            "        DataType::object::<Plan>(),\n",
            "        |parameter| {\n",
            "            parameter.required(true);\n",
            "        },\n",
            "    );\n",
            "    flow.returns(DataType::Boolean);\n",
            "    let first = flow.create_variable(\"First\", DataType::Boolean, mx(\"true\"));\n",
            "    let second = flow.create_variable(\"Second\", DataType::Boolean, mx(\"true\"));\n",
            "\n",
            "    flow.decision(\n",
            "        mx(\"$First\"),\n",
            "        |flow| {\n",
            "            flow.commit(&first);\n",
            "\n",
            "            flow.commit(&second);\n",
            "        },\n",
            "        |_| {},\n",
            "    );\n",
            "\n",
            // One activity's options are one thing: they stay together.
            "    let list = flow.retrieve(\"List\", Ref::<Plan>::new(), |retrieve| {\n",
            "        retrieve.xpath(\"[Open]\");\n",
            "        retrieve.first();\n",
            "    });\n",
            "\n",
            "    flow.return_with(mx(\"true\"));\n",
            "}\n",
        );
        assert_eq!(space_statements(source), expected);
        assert_eq!(
            space_statements(expected),
            expected,
            "spacing twice changes nothing"
        );
    }
}
