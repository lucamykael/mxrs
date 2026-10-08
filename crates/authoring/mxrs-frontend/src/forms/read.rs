//! The TSX that declares documents, read — never run — into them.

use std::collections::{HashMap, HashSet};

use mxrs_ir::{FormDecl, NativeDocument, NativeValue};
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrayExpressionElement, BindingPattern, Declaration, Expression,
    ImportDeclarationSpecifier, JSXAttributeItem, JSXAttributeName, JSXAttributeValue, JSXChild,
    JSXElement, JSXElementName, ObjectExpression, ObjectPropertyKind, Program, PropertyKey,
    Statement, UnaryOperator, VariableDeclarationKind,
};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span};

use super::write::{keys, property_default, unstated_shape};
use super::{
    CUSTOM_WIDGET, ELEMENTS_MODULE, FORMS_MODULE, Field, FieldDefault, Shape, Shapes, TEXT,
    TRANSLATION, Vocabulary, WIDGET_OBJECT, WIDGET_PROPERTY, WIDGET_VALUE, WIDGETS_FOLDER,
    WidgetDefinition, join,
};
use crate::FrontendError;

type Outcome<T> = Result<T, FrontendError>;

/// What a pointer holds until the document it names is found: this, then
/// the name.
const NAMED: char = '\u{0}';

struct File<'s> {
    path: &'s str,
    source: &'s str,
}

impl File<'_> {
    fn line(&self, span: Span) -> usize {
        let start = (span.start as usize).min(self.source.len());
        self.source[..start].matches('\n').count() + 1
    }

    fn refuse(&self, span: Span, expected: &str) -> FrontendError {
        let start = (span.start as usize).min(self.source.len());
        let end = (span.end as usize).clamp(start, self.source.len());
        let found: String = self.source[start..end].chars().take(60).collect();
        FrontendError::Unsupported {
            path: self.path.to_string(),
            line: self.line(span),
            expected: expected.to_string(),
            found,
        }
    }

    fn shape(&self, span: Span, detail: impl Into<String>) -> FrontendError {
        FrontendError::Shape {
            path: self.path.to_string(),
            line: self.line(span),
            detail: detail.into(),
        }
    }
}

fn parse<'a>(allocator: &'a Allocator, file: &File<'a>, tsx: bool) -> Outcome<Program<'a>> {
    let source_type = if tsx {
        SourceType::tsx()
    } else {
        SourceType::ts()
    };
    let parsed = Parser::new(allocator, file.source, source_type).parse();
    if let Some(error) = parsed.diagnostics.first() {
        return Err(crate::syntax_error(file.path, file.source, error));
    }
    Ok(parsed.program)
}

/// A whole number, as written.
fn whole(file: &File<'_>, expression: &Expression<'_>) -> Outcome<i64> {
    const EXACT: f64 = 9_007_199_254_740_992.0;
    let (number, span) = match expression.without_parentheses() {
        Expression::NumericLiteral(number) => (number.value, number.span),
        Expression::UnaryExpression(unary) if unary.operator == UnaryOperator::UnaryNegation => {
            match unary.argument.without_parentheses() {
                Expression::NumericLiteral(number) => (-number.value, unary.span),
                other => return Err(file.refuse(other.span(), "a number")),
            }
        }
        other => return Err(file.refuse(other.span(), "a number")),
    };
    if number.fract() != 0.0 || number.abs() >= EXACT {
        return Err(file.refuse(span, "a whole number within ±2^53"));
    }
    #[allow(clippy::cast_possible_truncation)]
    Ok(number as i64)
}

fn text(file: &File<'_>, expression: &Expression<'_>) -> Option<Outcome<String>> {
    Some(match expression.without_parentheses() {
        Expression::StringLiteral(text) => {
            if text.lone_surrogates {
                Err(file.refuse(text.span, "text without a lone surrogate"))
            } else {
                Ok(text.value.to_string())
            }
        }
        Expression::TemplateLiteral(template) if template.expressions.is_empty() => {
            let mut text = String::new();
            for quasi in &template.quasis {
                if quasi.lone_surrogates {
                    return Some(Err(file.refuse(quasi.span, "text without a lone surrogate")));
                }
                match &quasi.value.cooked {
                    Some(cooked) => text.push_str(cooked),
                    None => return Some(Err(file.refuse(quasi.span, "a valid escape"))),
                }
            }
            Ok(text)
        }
        _ => return None,
    })
}

/// The one argument of a call that takes one.
fn argument<'p, 'a>(
    file: &File<'_>,
    call: &'p oxc_ast::ast::CallExpression<'a>,
    expected: &str,
) -> Outcome<&'p Expression<'a>> {
    match call.arguments.as_slice() {
        [only] => only
            .as_expression()
            .ok_or_else(|| file.refuse(only.span(), expected)),
        _ => Err(file.refuse(call.span, expected)),
    }
}

/// Reads `src/mxrs/elements.ts`: the elements pages are written with.
pub fn read_elements(source: &str, path: &str) -> Result<Shapes, FrontendError> {
    let allocator = Allocator::default();
    let file = File { path, source };
    let program = parse(&allocator, &file, false)?;
    let mut shapes: Vec<Shape> = Vec::new();
    // Each element by its component, to name the type a default holds.
    let mut declared: HashSet<String> = HashSet::new();
    for statement in &program.body {
        let export = match statement {
            Statement::ImportDeclaration(_) => continue,
            Statement::ExportDeclaration(export) => export,
            other => return Err(file.refuse(other.span(), "`export const X = element(...)`")),
        };
        let Declaration::VariableDeclaration(declaration) = &export.declaration else {
            return Err(file.refuse(export.span, "`export const X = element(...)`"));
        };
        if declaration.kind != VariableDeclarationKind::Const {
            return Err(file.refuse(declaration.span, "`const`"));
        }
        for declarator in &declaration.declarations {
            let BindingPattern::BindingIdentifier(name) = &declarator.id else {
                return Err(file.refuse(declarator.span, "a name"));
            };
            let Some(Expression::CallExpression(call)) = &declarator.init else {
                return Err(file.refuse(declarator.span, "`element(type, defaults)`"));
            };
            let (ty, defaults, main) = match call.arguments.as_slice() {
                [ty, defaults] => (ty, defaults, None),
                [ty, defaults, main] => (ty, defaults, Some(main)),
                _ => return Err(file.refuse(call.span, "`element(type, defaults)`")),
            };
            let (
                Expression::Identifier(callee),
                Some(Expression::StringLiteral(ty)),
                Some(Expression::ObjectExpression(defaults)),
            ) = (&call.callee, ty.as_expression(), defaults.as_expression())
            else {
                return Err(file.refuse(call.span, "`element(type, defaults)`"));
            };
            if callee.name != "element" {
                return Err(file.refuse(call.callee.span(), "`element`"));
            }
            let typed = ty
                .value
                .split_once('$')
                .is_some_and(|(space, name)| is_name(space) && is_name(name));
            if !typed {
                return Err(file.refuse(ty.span, "a stored type: `Forms$DivContainer`"));
            }
            if !name.name.starts_with(|c: char| c.is_ascii_uppercase()) || !is_name(&name.name) {
                return Err(file.refuse(
                    name.span,
                    "a component's name: a capital, then letters, digits and `_`",
                ));
            }
            let mut fields: Vec<Field> = Vec::new();
            let mut children = None;
            for property in &defaults.properties {
                let ObjectPropertyKind::ObjectProperty(property) = property else {
                    return Err(file.refuse(property.span(), "a field, not a spread"));
                };
                if property.computed || property.method || property.shorthand {
                    return Err(file.refuse(property.span, "`field: default`"));
                }
                let Some(prop) = property.key.static_name() else {
                    return Err(file.refuse(property.key.span(), "a field's name"));
                };
                // A prop's field is its name with its capital back.
                let mut letters = prop.chars();
                let key = match letters.next() {
                    Some(first) if first.is_ascii_lowercase() => {
                        format!("{}{}", first.to_ascii_uppercase(), letters.as_str())
                    }
                    _ => prop.to_string(),
                };
                if super::prop_of(&key).as_deref() != Some(prop.as_ref()) {
                    return Err(file.refuse(property.key.span(), "a field's name as a prop"));
                }
                if fields.iter().any(|field| field.key == key) {
                    return Err(file.refuse(property.key.span(), "each field once"));
                }
                let value = property.value.without_parentheses();
                let default = match value {
                    Expression::NullLiteral(_) => FieldDefault::Value(NativeValue::Null),
                    Expression::BooleanLiteral(flag) => {
                        FieldDefault::Value(NativeValue::Bool(flag.value))
                    }
                    Expression::Identifier(element) if declared.contains(element.name.as_str()) => {
                        FieldDefault::Element(element.name.to_string())
                    }
                    Expression::Identifier(element) => {
                        return Err(file.refuse(element.span, "an element declared above"));
                    }
                    Expression::CallExpression(call)
                        if matches!(&call.callee, Expression::Identifier(callee) if callee.name == "binary")
                            && call.arguments.is_empty() =>
                    {
                        FieldDefault::Value(NativeValue::Binary(Vec::new()))
                    }
                    Expression::CallExpression(call) => {
                        let Expression::Identifier(callee) = &call.callee else {
                            return Err(file.refuse(call.span, "`list(n)` or `children(n)`"));
                        };
                        if !matches!(callee.name.as_str(), "long" | "list" | "children") {
                            return Err(
                                file.refuse(call.span, "`list(n)`, `children(n)` or `long(n)`")
                            );
                        }
                        let number = whole(&file, argument(&file, call, "one number")?)?;
                        match callee.name.as_str() {
                            "long" => FieldDefault::Value(NativeValue::Int64(number)),
                            "list" | "children" => {
                                let marker = i32::try_from(number)
                                    .map_err(|_| file.refuse(call.span, "a list's marker"))?;
                                if callee.name == "children" {
                                    if children.is_some() {
                                        return Err(file.shape(
                                            call.span,
                                            "an element has one list of children",
                                        ));
                                    }
                                    children = Some(key.clone());
                                }
                                FieldDefault::List(marker)
                            }
                            _ => {
                                return Err(
                                    file.refuse(call.span, "`list(n)`, `children(n)` or `long(n)`")
                                );
                            }
                        }
                    }
                    other => match text(&file, other) {
                        Some(text) => FieldDefault::Value(NativeValue::Text(text?)),
                        None => {
                            let number = whole(&file, other).map_err(|_| {
                                file.refuse(
                                    other.span(),
                                    "a default: null, a boolean, a number, a text, an element or a list",
                                )
                            })?;
                            FieldDefault::Value(NativeValue::Int32(i32::try_from(number).map_err(
                                |_| file.refuse(other.span(), "a 32-bit number, or `long(n)`"),
                            )?))
                        }
                    },
                };
                fields.push(Field {
                    key,
                    prop: prop.to_string(),
                    default,
                });
            }
            if !declared.insert(name.name.to_string()) {
                return Err(file.shape(declarator.span, "an element is declared once"));
            }
            // The field the element is mostly stated for, by its prop.
            let main = match main.map(|main| (main, main.as_expression())) {
                None => None,
                Some((_, Some(Expression::StringLiteral(prop)))) => Some(
                    fields
                        .iter()
                        .find(|field| field.prop == prop.value.as_str())
                        // A list is stated as a list, never as one thing.
                        .filter(|field| !matches!(field.default, FieldDefault::List(_)))
                        .map(|field| field.key.clone())
                        .ok_or_else(|| {
                            file.refuse(prop.span, "one of the element's fields that is no list")
                        })?,
                ),
                Some((main, _)) => {
                    return Err(file.refuse(main.span(), "the name of the element's main field"));
                }
            };
            shapes.push(Shape {
                ty: ty.value.to_string(),
                component: name.name.to_string(),
                fields,
                children,
                main,
            });
        }
    }
    Shapes::new(shapes).map_err(|detail| FrontendError::Shape {
        path: path.to_string(),
        line: 1,
        detail,
    })
}

/// What a field holds, when `value` is not of that kind: a field whose
/// default is a text holds texts, and so on. A field that holds nothing by
/// default says nothing of its kind.
fn kind_of(default: &FieldDefault, value: &NativeValue) -> Option<&'static str> {
    let fits = match default {
        FieldDefault::Value(NativeValue::Text(_)) => matches!(value, NativeValue::Text(_)),
        FieldDefault::Value(NativeValue::Bool(_)) => matches!(value, NativeValue::Bool(_)),
        FieldDefault::Value(NativeValue::Int32(_) | NativeValue::Int64(_)) => {
            matches!(value, NativeValue::Int32(_) | NativeValue::Int64(_))
        }
        FieldDefault::List(_) => matches!(value, NativeValue::List(..)),
        FieldDefault::Element(_) => {
            matches!(value, NativeValue::Document(_) | NativeValue::Null)
        }
        FieldDefault::Value(_) => true,
    };
    (!fits).then(|| kind_name(default))
}

/// What a field of this default holds, in words.
fn kind_name(default: &FieldDefault) -> &'static str {
    match default {
        FieldDefault::Value(NativeValue::Text(_)) => "a text",
        FieldDefault::Value(NativeValue::Bool(_)) => "true or false",
        FieldDefault::Value(_) => "a number",
        FieldDefault::List(_) => "a list",
        FieldDefault::Element(_) => "an element, or null",
    }
}

/// Whether `name` is one a model gives: letters, digits and `_`, not
/// opening with a digit.
fn is_name(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The helpers of `src/mxrs/forms.ts` a declaration's values call.
const HELPERS: [&str; 7] = [
    "long", "int", "named", "identity", "unset", "missing", "binary",
];

/// `expression` without what only TypeScript reads: parentheses, `as`,
/// `satisfies` and `!`.
fn plain<'p, 'a>(expression: &'p Expression<'a>) -> &'p Expression<'a> {
    let mut expression = expression;
    loop {
        expression = match expression {
            Expression::ParenthesizedExpression(inner) => &inner.expression,
            Expression::TSAsExpression(cast) => &cast.expression,
            Expression::TSSatisfiesExpression(check) => &check.expression,
            Expression::TSNonNullExpression(sure) => &sure.expression,
            other => return other,
        };
    }
}

/// What a prop is given: text between quotes, an expression, or an element.
enum Given<'p, 'a> {
    Text(String),
    Expression(&'p Expression<'a>),
    Element(&'p JSXElement<'a>),
}

/// What a name a file imports stands for.
enum Imported {
    Element(String),
    Widget(String),
    Helper(String),
    /// A file beside the page — `import thumbnail from "./Home.png"` — by
    /// the specifier it is imported with: binary data the page holds.
    File(String),
}

/// The bytes of a file a page imports, by the specifier it imports it with.
type Files<'f> = &'f dyn Fn(&str) -> Option<Vec<u8>>;

struct Reader<'v, 's> {
    file: File<'s>,
    files: Files<'v>,
    shapes: &'v Shapes,
    widgets: &'v [WidgetDefinition],
    imports: HashMap<String, Imported>,
    /// Each `named(...)` read, by the name it gave, with where it was.
    named: HashMap<String, Span>,
}

impl<'v> Reader<'v, '_> {
    fn import(&mut self, program: &Program<'_>) {
        for statement in &program.body {
            let Statement::ImportDeclaration(import) = statement else {
                continue;
            };
            let source = import.source.value.as_str();
            for specifier in import.specifiers.iter().flatten() {
                // A file beside the form, and only there.
                if let ImportDeclarationSpecifier::ImportDefaultSpecifier(default) = specifier
                    && let Some(file) = source.strip_prefix("./")
                    && !file.is_empty()
                    && !file.contains(['/', '\\'])
                    && file != ".."
                {
                    self.imports.insert(
                        default.local.name.to_string(),
                        Imported::File(source.to_string()),
                    );
                    continue;
                }
                let ImportDeclarationSpecifier::ImportSpecifier(specifier) = specifier else {
                    continue;
                };
                let imported = specifier.imported.name().to_string();
                let stands_for = if source == ELEMENTS_MODULE {
                    Imported::Element(imported)
                } else if source == FORMS_MODULE {
                    Imported::Helper(imported)
                } else if source == format!("@/{WIDGETS_FOLDER}/{imported}") {
                    // A widget is the one export of the file named for it.
                    Imported::Widget(imported)
                } else {
                    continue;
                };
                self.imports
                    .insert(specifier.local.name.to_string(), stands_for);
            }
        }
    }

    /// Why a call is not a value: a helper the file does not import, or
    /// no helper at all.
    fn unknown_call(&self, call: &oxc_ast::ast::CallExpression<'_>) -> FrontendError {
        if let Expression::Identifier(callee) = &call.callee
            && HELPERS.contains(&callee.name.as_str())
            && !self.imports.contains_key(callee.name.as_str())
        {
            return self.file.shape(
                call.span,
                format!(
                    "`{}` is not imported: import it from \"{FORMS_MODULE}\"",
                    callee.name
                ),
            );
        }
        self.file.refuse(
            call.span,
            "`long(n)`, `int(n)`, `named(name)`, `identity(id)` or `binary()`",
        )
    }

    /// The helper of `src/mxrs/forms.ts` a call calls.
    fn helper(&self, call: &oxc_ast::ast::CallExpression<'_>) -> Option<&str> {
        let Expression::Identifier(callee) = &call.callee else {
            return None;
        };
        match self.imports.get(callee.name.as_str())? {
            Imported::Helper(name) => Some(name),
            _ => None,
        }
    }

    fn element(&mut self, jsx: &JSXElement<'_>, path: &str) -> Outcome<NativeDocument> {
        let tag = match &jsx.opening_element.name {
            JSXElementName::IdentifierReference(name) => name.name.as_str(),
            JSXElementName::Identifier(name) => name.name.as_str(),
            other => return Err(self.file.refuse(other.span(), "an element")),
        };
        match self.imports.get(tag) {
            Some(Imported::Element(component)) => {
                let shapes = self.shapes;
                let Some(shape) = shapes.component(component) else {
                    return Err(self.file.refuse(
                        jsx.opening_element.name.span(),
                        "an element src/mxrs/elements.ts declares",
                    ));
                };
                if !path.is_empty() && super::declarer(&shape.ty).is_some() {
                    return Err(self.file.shape(
                        jsx.opening_element.name.span(),
                        format!(
                            "<{}> is a file of its own, not a part of another",
                            shape.component
                        ),
                    ));
                }
                if shape.ty == CUSTOM_WIDGET {
                    return Err(self.file.shape(
                        jsx.opening_element.name.span(),
                        "a pluggable widget is used by the name src/widgets defines it with",
                    ));
                }
                let base = shapes.default_for(shape).clone();
                self.fill(jsx, shape, base, path, &[], &[])
            }
            Some(Imported::Widget(name)) => {
                let widgets = self.widgets;
                let Some(definition) = widgets.iter().find(|widget| &widget.name == name) else {
                    return Err(self.file.refuse(
                        jsx.opening_element.name.span(),
                        "a widget src/widgets declares",
                    ));
                };
                self.widget(jsx, definition, path)
            }
            _ => Err(self.file.refuse(
                jsx.opening_element.name.span(),
                "an element imported from @/mxrs/elements, or a widget from @/widgets",
            )),
        }
    }

    /// `base` with what the element's props and children state. The props
    /// in `apart` are read by the caller, and those in `implied` are not a
    /// declaration's to state.
    fn fill(
        &mut self,
        jsx: &JSXElement<'_>,
        shape: &Shape,
        mut document: NativeDocument,
        path: &str,
        apart: &[&str],
        implied: &[&str],
    ) -> Outcome<NativeDocument> {
        let mut seen: HashSet<String> = HashSet::new();
        for attribute in &jsx.opening_element.attributes {
            let JSXAttributeItem::Attribute(attribute) = attribute else {
                return Err(self.file.refuse(attribute.span(), "a prop, not a spread"));
            };
            let JSXAttributeName::Identifier(name) = &attribute.name else {
                return Err(self.file.refuse(attribute.name.span(), "a prop's name"));
            };
            let prop = name.name.as_str();
            if !seen.insert(prop.to_string()) {
                return Err(self.file.refuse(attribute.span, "each prop once"));
            }
            if apart.contains(&prop) {
                continue;
            }
            let Some(field) = shape.field_of_prop(prop) else {
                return Err(self.file.shape(
                    attribute.span,
                    format!("<{}> has no prop `{prop}`", shape.component),
                ));
            };
            if implied.contains(&field.key.as_str()) {
                return Err(self.file.shape(
                    attribute.span,
                    format!("`{prop}` follows from where the element is; it is not stated"),
                ));
            }
            if shape.children.as_deref() == Some(&field.key) {
                return Err(self.file.shape(
                    attribute.span,
                    format!("`{prop}` is written as the element's children"),
                ));
            }
            let nested = join(path, &field.key);
            let given = match &attribute.value {
                None => {
                    return Err(self
                        .file
                        .refuse(attribute.span, "a value: `{true}` for a flag"));
                }
                Some(JSXAttributeValue::StringLiteral(text)) => {
                    if text.lone_surrogates || text.value.contains('&') {
                        return Err(self.file.refuse(
                            text.span,
                            "text without `&` between quotes: write it as `{\"...\"}`",
                        ));
                    }
                    Given::Text(text.value.to_string())
                }
                Some(JSXAttributeValue::ExpressionContainer(container)) => {
                    let Some(expression) = container.expression.as_expression() else {
                        return Err(self.file.refuse(container.span, "a value"));
                    };
                    Given::Expression(expression)
                }
                Some(JSXAttributeValue::Element(element)) => Given::Element(element),
                Some(JSXAttributeValue::Fragment(fragment)) => {
                    return Err(self.file.refuse(fragment.span, "one element"));
                }
            };
            let held = document.get(&field.key).cloned();
            let value = self.given(given, field, held.as_ref(), attribute.span, &nested, prop)?;
            document.set(&field.key, value);
        }
        let mut children = Vec::new();
        for child in &jsx.children {
            match child {
                JSXChild::Element(element) => children.push(element),
                JSXChild::Text(text) if text.value.trim().is_empty() => {}
                JSXChild::ExpressionContainer(container)
                    if container.expression.as_expression().is_none() => {}
                other => return Err(self.file.refuse(other.span(), "an element")),
            }
        }
        if !children.is_empty() {
            let held = shape.children.as_deref().and_then(|key| {
                match shape.field(key).map(|field| &field.default) {
                    Some(FieldDefault::List(marker)) => Some((key, *marker)),
                    _ => None,
                }
            });
            let Some((key, marker)) = held else {
                return Err(self.file.shape(
                    children[0].span,
                    format!("<{}> holds no children", shape.component),
                ));
            };
            let nested = join(path, key);
            let mut items = Vec::with_capacity(children.len());
            for (index, child) in children.into_iter().enumerate() {
                items.push(NativeValue::Document(
                    self.element(child, &format!("{nested}[{index}]"))?,
                ));
            }
            document.set(key, NativeValue::List(marker, items));
        }
        Ok(document)
    }

    /// What a prop states for `field`, which holds `held` before it says
    /// anything. A value that is no element, where an element with a main
    /// field is held, is that field of it.
    #[allow(clippy::too_many_arguments)]
    fn given(
        &mut self,
        given: Given<'_, '_>,
        field: &Field,
        held: Option<&NativeValue>,
        at: Span,
        path: &str,
        named: &str,
    ) -> Outcome<NativeValue> {
        let shapes = self.shapes;
        // What TypeScript alone reads — a cast, parentheses — says nothing.
        let given = match given {
            Given::Expression(expression) => Given::Expression(plain(expression)),
            other => other,
        };
        let expression = match &given {
            Given::Expression(expression) => Some(*expression),
            _ => None,
        };
        // Where the prop states a field of the element it holds, the field
        // is named beside the prop.
        let holds = |expected: &str| {
            if field.prop == named {
                format!("`{named}` holds {expected}")
            } else {
                format!(
                    "`{named}` states its `{}`, which holds {expected}",
                    field.prop
                )
            }
        };
        let whole = matches!(given, Given::Element(_))
            || matches!(
                expression,
                Some(Expression::JSXElement(_) | Expression::NullLiteral(_))
            );
        if !whole
            && let (Some((_, main)), Some(NativeValue::Document(element))) =
                (shapes.main_of(held), held)
        {
            let mut element = element.clone();
            let inner = self.given(
                given,
                main,
                element.get(&main.key).cloned().as_ref(),
                at,
                &join(path, &main.key),
                named,
            )?;
            element.set(&main.key, inner);
            return Ok(NativeValue::Document(element));
        }
        if matches!(expression, Some(Expression::ObjectExpression(_)))
            && !super::texts_fit(shapes, field, held)
        {
            return Err(self.file.shape(
                at,
                holds(&format!(
                    "{}, not texts by language",
                    kind_name(&field.default)
                )),
            ));
        }
        let value = match given {
            Given::Text(text) => NativeValue::Text(text),
            Given::Element(element) => NativeValue::Document(self.element(element, path)?),
            Given::Expression(expression) => self.value(expression, Some(field), path)?,
        };
        if let Some(expected) = kind_of(&field.default, &value) {
            return Err(self.file.shape(at, holds(expected)));
        }
        Ok(value)
    }

    /// The objects a property holds, each by its own properties.
    #[allow(clippy::too_many_arguments)]
    fn objects(
        &mut self,
        stated: &Expression<'_>,
        value_type: &NativeDocument,
        value_type_path: &str,
        marker: Option<i32>,
        path: &str,
        definition: &WidgetDefinition,
        keys: &str,
    ) -> Outcome<NativeValue> {
        let Expression::ArrayExpression(array) = stated else {
            return Err(self.file.refuse(stated.span(), "a list of objects"));
        };
        let (Some(NativeValue::Document(nested_type)), Some(marker)) =
            (value_type.get("ObjectType"), marker)
        else {
            return Err(self
                .file
                .shape(array.span, "this property holds no objects"));
        };
        let mut items = Vec::with_capacity(array.elements.len());
        for (index, element) in array.elements.iter().enumerate() {
            let Some(Expression::ObjectExpression(object)) =
                element.as_expression().map(Expression::without_parentheses)
            else {
                return Err(self
                    .file
                    .refuse(element.span(), "an object: its properties by their keys"));
            };
            items.push(NativeValue::Document(self.object(
                nested_type,
                &format!("{value_type_path}.ObjectType"),
                Some(object),
                object.span,
                &format!("{path}.Objects[{index}]"),
                definition,
                keys,
            )?));
        }
        Ok(NativeValue::List(marker, items))
    }

    fn value(
        &mut self,
        expression: &Expression<'_>,
        field: Option<&Field>,
        path: &str,
    ) -> Outcome<NativeValue> {
        let expression = expression.without_parentheses();
        if let Some(text) = text(&self.file, expression) {
            return Ok(NativeValue::Text(text?));
        }
        let wide = matches!(
            field.map(|field| &field.default),
            Some(FieldDefault::Value(NativeValue::Int64(_)))
        );
        Ok(match expression {
            Expression::NullLiteral(_) => NativeValue::Null,
            Expression::BooleanLiteral(flag) => NativeValue::Bool(flag.value),
            Expression::Identifier(name) if matches!(self.imports.get(name.name.as_str()), Some(Imported::Helper(helper)) if helper == "unset") => {
                NativeValue::Identity(NativeValue::NOTHING.to_string())
            }
            Expression::Identifier(name)
                if matches!(
                    self.imports.get(name.name.as_str()),
                    Some(Imported::File(_))
                ) =>
            {
                let Some(Imported::File(specifier)) = self.imports.get(name.name.as_str()) else {
                    unreachable!("matched a file just above");
                };
                let bytes = (self.files)(specifier).ok_or_else(|| {
                    self.file.shape(
                        name.span,
                        format!("`{specifier}` is no file beside the page"),
                    )
                })?;
                NativeValue::Binary(bytes)
            }
            Expression::NumericLiteral(_) | Expression::UnaryExpression(_) => {
                let number = whole(&self.file, expression)?;
                if wide {
                    NativeValue::Int64(number)
                } else {
                    NativeValue::Int32(i32::try_from(number).map_err(|_| {
                        self.file
                            .refuse(expression.span(), "a 32-bit number, or `long(n)`")
                    })?)
                }
            }
            Expression::JSXElement(element) => NativeValue::Document(self.element(element, path)?),
            Expression::ArrayExpression(array) => {
                let Some(FieldDefault::List(marker)) = field.map(|field| &field.default) else {
                    return Err(self.file.shape(
                        array.span,
                        format!(
                            "`{}` does not hold a list",
                            field.map_or(path, |field| field.prop.as_str())
                        ),
                    ));
                };
                let mut items = Vec::with_capacity(array.elements.len());
                for (index, element) in array.elements.iter().enumerate() {
                    let element = match element {
                        ArrayExpressionElement::SpreadElement(spread) => {
                            return Err(self.file.refuse(spread.span, "a value, not a spread"));
                        }
                        ArrayExpressionElement::Elision(hole) => {
                            return Err(self.file.refuse(hole.span, "a value, not a hole"));
                        }
                        other => other
                            .as_expression()
                            .expect("an element that is neither a spread nor a hole"),
                    };
                    items.push(self.value(element, None, &format!("{path}[{index}]"))?);
                }
                NativeValue::List(*marker, items)
            }
            Expression::ObjectExpression(object) => NativeValue::Document(self.texts(object)?),
            Expression::CallExpression(call) => match self.helper(call) {
                Some("binary") if call.arguments.is_empty() => NativeValue::Binary(Vec::new()),
                Some("long") => {
                    NativeValue::Int64(whole(&self.file, argument(&self.file, call, "a number")?)?)
                }
                Some("int") => {
                    let number = whole(&self.file, argument(&self.file, call, "a number")?)?;
                    NativeValue::Int32(
                        i32::try_from(number)
                            .map_err(|_| self.file.refuse(call.span, "a 32-bit number"))?,
                    )
                }
                Some("identity") => {
                    let id = argument(&self.file, call, "an identity")?;
                    let Some(text) = text(&self.file, id) else {
                        return Err(self.file.refuse(id.span(), "an identity"));
                    };
                    let text = text?;
                    let shaped = text.len() == 36
                        && text.chars().enumerate().all(|(index, c)| {
                            if matches!(index, 8 | 13 | 18 | 23) {
                                c == '-'
                            } else {
                                c.is_ascii_digit() || ('a'..='f').contains(&c)
                            }
                        });
                    if !shaped {
                        return Err(self.file.refuse(
                            id.span(),
                            "an identity: 8-4-4-4-12 digits and letters a to f",
                        ));
                    }
                    NativeValue::Identity(text)
                }
                Some("named") => {
                    let name = argument(&self.file, call, "a name")?;
                    let Some(name) = text(&self.file, name) else {
                        return Err(self.file.refuse(name.span(), "a name"));
                    };
                    let name = name?;
                    self.named.entry(name.clone()).or_insert(call.span);
                    NativeValue::Pointer(format!("{NAMED}{name}"))
                }
                _ => return Err(self.unknown_call(call)),
            },
            Expression::Identifier(name)
                if HELPERS.contains(&name.name.as_str())
                    && !self.imports.contains_key(name.name.as_str()) =>
            {
                return Err(self.file.shape(
                    name.span,
                    format!(
                        "`{}` is not imported: import it from \"{FORMS_MODULE}\"",
                        name.name
                    ),
                ));
            }
            Expression::TSAsExpression(cast) => self.value(&cast.expression, field, path)?,
            Expression::TSSatisfiesExpression(check) => {
                self.value(&check.expression, field, path)?
            }
            other => {
                return Err(self.file.refuse(
                    other.span(),
                    "a value: a text, number, boolean, null, element, list or texts by language",
                ));
            }
        })
    }

    /// `{ en_US: "..." }`: a text, as its translations by language code.
    fn texts(&mut self, object: &ObjectExpression<'_>) -> Outcome<NativeDocument> {
        let Some(marker) = self.shapes.text_marker() else {
            return Err(self.file.shape(
                object.span,
                "src/mxrs/elements.ts declares no Text made of Translations",
            ));
        };
        let mut items = Vec::new();
        for (language, value) in self.entries(object)? {
            let Some(translated) = text(&self.file, value) else {
                return Err(self.file.refuse(value.span(), "a text"));
            };
            let mut translation = NativeDocument::new(TRANSLATION);
            translation.set("LanguageCode", language);
            translation.set("Text", translated?);
            items.push(NativeValue::Document(translation));
        }
        Ok(NativeDocument::new(TEXT).with("Items", NativeValue::List(marker, items)))
    }

    /// The entries of an object literal: each key once, with its value.
    fn entries<'p, 'a>(
        &self,
        object: &'p ObjectExpression<'a>,
    ) -> Outcome<Vec<(String, &'p Expression<'a>)>> {
        let mut entries: Vec<(String, &Expression<'_>)> = Vec::new();
        for property in &object.properties {
            let ObjectPropertyKind::ObjectProperty(property) = property else {
                return Err(self.file.refuse(property.span(), "an entry, not a spread"));
            };
            if property.computed || property.method || property.shorthand {
                return Err(self.file.refuse(property.span, "`key: value`"));
            }
            if let PropertyKey::StringLiteral(key) = &property.key
                && key.lone_surrogates
            {
                return Err(self.file.refuse(key.span, "a key without a lone surrogate"));
            }
            let Some(key) = property.key.static_name() else {
                return Err(self.file.refuse(property.key.span(), "a key"));
            };
            if entries.iter().any(|(known, _)| known.as_str() == key) {
                return Err(self.file.refuse(property.key.span(), "each key once"));
            }
            entries.push((key.to_string(), &property.value));
        }
        Ok(entries)
    }

    /// The prop `prop` of `jsx`, when it states an expression.
    fn prop<'p, 'a>(
        &self,
        jsx: &'p JSXElement<'a>,
        prop: &str,
    ) -> Outcome<Option<&'p Expression<'a>>> {
        for attribute in &jsx.opening_element.attributes {
            let JSXAttributeItem::Attribute(attribute) = attribute else {
                continue;
            };
            let JSXAttributeName::Identifier(name) = &attribute.name else {
                continue;
            };
            if name.name != prop {
                continue;
            }
            return match &attribute.value {
                Some(JSXAttributeValue::ExpressionContainer(container)) => container
                    .expression
                    .as_expression()
                    .map(|expression| Some(expression.without_parentheses()))
                    .ok_or_else(|| self.file.refuse(container.span, "a value")),
                _ => Err(self
                    .file
                    .refuse(attribute.span, &format!("`{prop}={{...}}`"))),
            };
        }
        Ok(None)
    }

    fn widget(
        &mut self,
        jsx: &JSXElement<'_>,
        definition: &WidgetDefinition,
        path: &str,
    ) -> Outcome<NativeDocument> {
        let shapes = self.shapes;
        let undeclared = |file: &File<'_>| {
            file.shape(
                jsx.opening_element.span,
                "src/mxrs/elements.ts does not declare what a pluggable widget is stored as",
            )
        };
        let Some(shape) = shapes.shape(CUSTOM_WIDGET) else {
            return Err(undeclared(&self.file));
        };
        let Some(NativeValue::Document(object_type)) = definition.ty.get("ObjectType") else {
            return Err(self.file.shape(
                jsx.opening_element.span,
                format!("{} defines no properties", definition.name),
            ));
        };
        if shape.field("Type").is_none()
            || shape.field("Object").is_none()
            || shape.field_of_prop("properties").is_some()
        {
            return Err(undeclared(&self.file));
        }
        let base = shapes
            .default_document(CUSTOM_WIDGET)
            .expect("a shape has a default")
            .clone();
        let mut document =
            self.fill(jsx, shape, base, path, &["properties"], &["Type", "Object"])?;
        let properties = match self.prop(jsx, "properties")? {
            Some(Expression::ObjectExpression(object)) => Some(&**object),
            Some(other) => {
                return Err(self
                    .file
                    .refuse(other.span(), "the properties, by their keys"));
            }
            None => None,
        };
        let object = self.object(
            object_type,
            &format!("{}.ObjectType", join(path, "Type")),
            properties,
            jsx.opening_element.span,
            &join(path, "Object"),
            definition,
            "",
        )?;
        document.set("Type", NativeValue::Document(definition.ty.clone()));
        document.set("Object", NativeValue::Document(object));
        Ok(document)
    }

    /// The object of type `object_type`, at `type_path`, whose properties
    /// are the ones `given` states and the defaults of the rest.
    #[allow(clippy::too_many_arguments)]
    fn object(
        &mut self,
        object_type: &NativeDocument,
        type_path: &str,
        given: Option<&ObjectExpression<'_>>,
        at: Span,
        path: &str,
        definition: &WidgetDefinition,
        prefix: &str,
    ) -> Outcome<NativeDocument> {
        let shapes = self.shapes;
        let undeclared = |file: &File<'_>| {
            file.shape(
                at,
                "src/mxrs/elements.ts does not declare what a widget's properties are stored as",
            )
        };
        let (Some(mut object), Some(property_base), Some(value_shape), Some(object_shape)) = (
            shapes.default_document(WIDGET_OBJECT).cloned(),
            shapes.default_document(WIDGET_PROPERTY),
            shapes.shape(WIDGET_VALUE),
            shapes.shape(WIDGET_OBJECT),
        ) else {
            return Err(undeclared(&self.file));
        };
        let Some(FieldDefault::List(marker)) =
            object_shape.field("Properties").map(|field| &field.default)
        else {
            return Err(undeclared(&self.file));
        };
        if object.get("TypePointer").is_none()
            || property_base.get("TypePointer").is_none()
            || property_base.get("Value").is_none()
        {
            return Err(undeclared(&self.file));
        }
        let mut entries = match given {
            Some(given) => self.entries(given)?,
            None => Vec::new(),
        };
        let types: &[NativeValue] = match object_type.get("PropertyTypes") {
            Some(NativeValue::List(_, types)) => types,
            _ => &[],
        };
        let mut properties = Vec::with_capacity(types.len());
        for (index, property_type) in types.iter().enumerate() {
            let property_type_path = format!("{type_path}.PropertyTypes[{index}]");
            let value_type_path = format!("{property_type_path}.ValueType");
            let value_path = format!("{path}.Properties[{}].Value", properties.len());
            let malformed = |file: &File<'_>| {
                file.shape(at, "the widget's definition has a property without a type")
            };
            let NativeValue::Document(property_type) = property_type else {
                return Err(malformed(&self.file));
            };
            let (Some(key), Some(NativeValue::Document(value_type))) = (
                property_type.text("PropertyKey"),
                property_type.get("ValueType"),
            ) else {
                return Err(malformed(&self.file));
            };
            let defined = definition.defaults.get(&keys(prefix, key));
            let Some(base) = unstated_shape(shapes, defined).and_then(|shape| {
                property_default(shapes, shape, value_type, &value_type_path, defined)
            }) else {
                return Err(undeclared(&self.file));
            };
            let stated = entries
                .iter()
                .position(|(known, _)| known == key)
                .map(|position| plain(entries.remove(position).1));
            let value = match stated {
                // A property the object does not store.
                Some(Expression::Identifier(name)) if matches!(self.imports.get(name.name.as_str()), Some(Imported::Helper(helper)) if helper == "missing") =>
                {
                    continue;
                }
                None => base,
                Some(Expression::JSXElement(element)) if self.is_value(element) => self
                    .widget_value(
                        element,
                        value_type,
                        &value_type_path,
                        &value_path,
                        definition,
                        &keys(prefix, key),
                    )?,
                // Anything else is the one thing a property of this type
                // is for.
                Some(other) => {
                    let main = super::property_main(value_type)
                        .and_then(|main| unstated_shape(shapes, defined)?.field(main));
                    let Some(main) = main else {
                        return Err(self
                            .file
                            .refuse(other.span(), &format!("a <{}>", value_shape.component)));
                    };
                    let mut value = base;
                    let held = super::property_held(shapes, &main.key, value.get(&main.key));
                    let said = match other {
                        stated if main.key == "Objects" => {
                            let marker = match &held {
                                Some(NativeValue::List(marker, _)) => Some(*marker),
                                _ => None,
                            };
                            self.objects(
                                stated,
                                value_type,
                                &value_type_path,
                                marker,
                                &value_path,
                                definition,
                                &keys(prefix, key),
                            )?
                        }
                        Expression::BooleanLiteral(flag)
                            if value_type.text("Type") == Some("Boolean") =>
                        {
                            NativeValue::Text(flag.value.to_string())
                        }
                        // The widgets a property holds: one alone, or a list.
                        stated if main.key == "Widgets" => {
                            let Some(NativeValue::List(marker, _)) = held else {
                                return Err(self
                                    .file
                                    .shape(stated.span(), "this property holds no widgets"));
                            };
                            let elements: Vec<&Expression<'_>> = match stated {
                                Expression::ArrayExpression(array) => array
                                    .elements
                                    .iter()
                                    .map(|element| {
                                        element.as_expression().ok_or_else(|| {
                                            self.file.refuse(element.span(), "a widget")
                                        })
                                    })
                                    .collect::<Outcome<_>>()?,
                                one => vec![one],
                            };
                            let mut widgets = Vec::with_capacity(elements.len());
                            for (index, element) in elements.into_iter().enumerate() {
                                let Expression::JSXElement(element) = element.without_parentheses()
                                else {
                                    return Err(self.file.refuse(element.span(), "a widget"));
                                };
                                widgets.push(NativeValue::Document(self.element(
                                    element,
                                    &format!("{value_path}.Widgets[{index}]"),
                                )?));
                            }
                            NativeValue::List(marker, widgets)
                        }
                        stated => {
                            // The property is what the page names, not the
                            // field of its value it is stated in.
                            let named = Field {
                                prop: key.to_string(),
                                ..main.clone()
                            };
                            self.given(
                                Given::Expression(stated),
                                &named,
                                held.as_ref(),
                                stated.span(),
                                &join(&value_path, &main.key),
                                key,
                            )?
                        }
                    };
                    if let Some(expected) = super::property_kind(&main.key, &said) {
                        return Err(self
                            .file
                            .shape(other.span(), format!("`{key}` holds {expected}")));
                    }
                    value.set(&main.key, said);
                    value
                }
            };
            let mut property = property_base.clone();
            property.set("TypePointer", NativeValue::Pointer(property_type_path));
            property.set("Value", NativeValue::Document(value));
            properties.push(NativeValue::Document(property));
        }
        if let Some((key, value)) = entries.first() {
            return Err(self
                .file
                .shape(value.span(), format!("the widget has no property `{key}`")));
        }
        object.set("Properties", NativeValue::List(*marker, properties));
        object.set("TypePointer", NativeValue::Pointer(type_path.to_string()));
        Ok(object)
    }

    /// Whether `jsx` is a property's value element, stated in full.
    fn is_value(&self, jsx: &JSXElement<'_>) -> bool {
        let tag = match &jsx.opening_element.name {
            JSXElementName::IdentifierReference(name) => name.name.as_str(),
            JSXElementName::Identifier(name) => name.name.as_str(),
            _ => return false,
        };
        matches!(
            self.imports.get(tag),
            Some(Imported::Element(component))
                if self.shapes.component(component).is_some_and(|shape| shape.ty == WIDGET_VALUE)
        )
    }

    fn widget_value(
        &mut self,
        jsx: &JSXElement<'_>,
        value_type: &NativeDocument,
        value_type_path: &str,
        path: &str,
        definition: &WidgetDefinition,
        keys: &str,
    ) -> Outcome<NativeDocument> {
        let shapes = self.shapes;
        let tag = match &jsx.opening_element.name {
            JSXElementName::IdentifierReference(name) => name.name.as_str(),
            JSXElementName::Identifier(name) => name.name.as_str(),
            other => return Err(self.file.refuse(other.span(), "an element")),
        };
        // The value element, or the one of its type stored another way.
        let shape = match self.imports.get(tag) {
            Some(Imported::Element(component)) => shapes
                .component(component)
                .filter(|shape| shape.ty == WIDGET_VALUE),
            _ => None,
        };
        let Some(shape) = shape else {
            return Err(self.file.refuse(
                jsx.opening_element.name.span(),
                "a property's value element",
            ));
        };
        let Some(base) = property_default(
            shapes,
            shape,
            value_type,
            value_type_path,
            definition.defaults.get(keys),
        ) else {
            return Err(self.file.shape(
                jsx.opening_element.span,
                "src/mxrs/elements.ts does not declare what a widget's properties are stored as",
            ));
        };
        let objects_prop = shape.field("Objects").map(|field| field.prop.clone());
        let apart: Vec<&str> = objects_prop.as_deref().into_iter().collect();
        let mut document = self.fill(jsx, shape, base, path, &apart, &["TypePointer"])?;
        let stated = match &objects_prop {
            Some(prop) => self.prop(jsx, prop)?,
            None => None,
        };
        if let Some(stated) = stated {
            let marker = match document.get("Objects") {
                Some(NativeValue::List(marker, _)) => Some(*marker),
                _ => None,
            };
            let objects = self.objects(
                stated,
                value_type,
                value_type_path,
                marker,
                path,
                definition,
                keys,
            )?;
            document.set("Objects", objects);
        }
        Ok(document)
    }

    /// Gives each `named(...)` the place of the one document of that name.
    fn resolve(&self, root: &mut NativeDocument) -> Outcome<()> {
        if self.named.is_empty() {
            return Ok(());
        }
        let mut places: HashMap<String, Vec<String>> = HashMap::new();
        super::walk(root, "", &mut |path, document| {
            if let Some(name) = document.text("Name")
                && self.named.contains_key(name)
            {
                places
                    .entry(name.to_string())
                    .or_default()
                    .push(path.to_string());
            }
        });
        for (name, span) in &self.named {
            match places.get(name).map(Vec::as_slice) {
                Some([_]) => {}
                Some(_) => {
                    return Err(self
                        .file
                        .shape(*span, format!("several elements are named `{name}`")));
                }
                None => {
                    return Err(self
                        .file
                        .shape(*span, format!("no element is named `{name}`")));
                }
            }
        }
        fn replace(document: &mut NativeDocument, places: &HashMap<String, Vec<String>>) {
            for (_, value) in &mut document.fields {
                replace_value(value, places);
            }
        }
        fn replace_value(value: &mut NativeValue, places: &HashMap<String, Vec<String>>) {
            match value {
                NativeValue::Pointer(target) => {
                    if let Some(name) = target.strip_prefix(NAMED) {
                        *target = places[name][0].clone();
                    }
                }
                NativeValue::Document(document) => replace(document, places),
                NativeValue::List(_, items) => {
                    for item in items {
                        replace_value(item, places);
                    }
                }
                _ => {}
            }
        }
        replace(root, &places);
        Ok(())
    }
}

/// Reads a file of `src/widgets/`: the definition of the pluggable widget
/// it exports.
pub fn read_widget(
    source: &str,
    path: &str,
    shapes: &Shapes,
) -> Result<WidgetDefinition, FrontendError> {
    let allocator = Allocator::default();
    let file = File { path, source };
    let program = parse(&allocator, &file, true)?;
    // A widget's definition imports no file.
    let mut reader = Reader {
        file,
        files: &|_: &str| None,
        shapes,
        widgets: &[],
        imports: HashMap::new(),
        named: HashMap::new(),
    };
    reader.import(&program);
    let mut found = None;
    for statement in &program.body {
        let export = match statement {
            Statement::ImportDeclaration(_) => continue,
            Statement::ExportDeclaration(export) => export,
            other => {
                return Err(reader
                    .file
                    .refuse(other.span(), "`export const X = widget(...)`"));
            }
        };
        let expected = "`export const X = widget(<CustomWidgetType ... />)`";
        let Declaration::VariableDeclaration(declaration) = &export.declaration else {
            return Err(reader.file.refuse(export.span, expected));
        };
        let [declarator] = declaration.declarations.as_slice() else {
            return Err(reader.file.refuse(declaration.span, expected));
        };
        let (BindingPattern::BindingIdentifier(name), Some(Expression::CallExpression(call))) =
            (&declarator.id, &declarator.init)
        else {
            return Err(reader.file.refuse(declarator.span, expected));
        };
        if reader.helper(call) != Some("widget") || found.is_some() {
            return Err(reader.file.refuse(call.span, expected));
        }
        let (element, stated) = match call.arguments.as_slice() {
            [element] => (element, None),
            [element, stated] => (element, Some(stated)),
            _ => return Err(reader.file.refuse(call.span, expected)),
        };
        let Some(Expression::JSXElement(element)) = argument_expression(element) else {
            return Err(reader.file.refuse(call.span, expected));
        };
        let ty = reader.element(element, "")?;
        if ty.ty != "CustomWidgets$CustomWidgetType" {
            return Err(reader
                .file
                .shape(element.span, "a widget's definition is a CustomWidgetType"));
        }
        let mut definition = WidgetDefinition {
            name: name.name.to_string(),
            ty,
            defaults: std::collections::BTreeMap::new(),
        };
        if let Some(stated) = stated {
            let Some(Expression::ObjectExpression(stated)) = argument_expression(stated) else {
                return Err(reader
                    .file
                    .refuse(stated.span(), "its properties' defaults, by their keys"));
            };
            for (key, value) in reader.entries(stated)? {
                if definition.value_type(&key).is_none() {
                    return Err(reader
                        .file
                        .shape(value.span(), format!("the widget has no property `{key}`")));
                }
                let Expression::JSXElement(value) = value.without_parentheses() else {
                    return Err(reader
                        .file
                        .refuse(value.span(), "a property's value element"));
                };
                let tag = match &value.opening_element.name {
                    JSXElementName::IdentifierReference(name) => name.name.as_str(),
                    JSXElementName::Identifier(name) => name.name.as_str(),
                    other => return Err(reader.file.refuse(other.span(), "an element")),
                };
                let shape = match reader.imports.get(tag) {
                    Some(Imported::Element(component)) => shapes
                        .component(component)
                        .filter(|shape| shape.ty == WIDGET_VALUE),
                    _ => None,
                };
                let Some(shape) = shape else {
                    return Err(reader.file.refuse(
                        value.opening_element.name.span(),
                        "a property's value element",
                    ));
                };
                let base = shapes.default_for(shape).clone();
                let read = reader.fill(value, shape, base, "", &[], &["TypePointer"])?;
                definition.defaults.insert(key, read);
            }
        }
        found = Some(definition);
    }
    found.ok_or_else(|| FrontendError::Syntax {
        path: path.to_string(),
        detail: "it declares no widget: `export const X = widget(...)`".to_string(),
    })
}

/// Reads the file that declares a page, layout or snippet: its module and
/// its document.
pub fn read_form(
    source: &str,
    path: &str,
    vocabulary: &Vocabulary,
) -> Result<(String, FormDecl), FrontendError> {
    // A file a page imports is beside it.
    let folder = std::path::Path::new(path)
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();
    read_form_with(source, path, vocabulary, &|specifier: &str| {
        std::fs::read(folder.join(specifier)).ok()
    })
}

/// [`read_form`], the files the page imports given by `files` rather
/// than read beside it.
pub fn read_form_with(
    source: &str,
    path: &str,
    vocabulary: &Vocabulary,
    files: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Result<(String, FormDecl), FrontendError> {
    let allocator = Allocator::default();
    let file = File { path, source };
    let program = parse(&allocator, &file, true)?;
    let mut reader = Reader {
        file,
        files,
        shapes: &vocabulary.shapes,
        widgets: &vocabulary.widgets,
        imports: HashMap::new(),
        named: HashMap::new(),
    };
    reader.import(&program);
    let expected = "`export default page(\"Module\", <Page ... />)`";
    let mut found = None;
    for statement in &program.body {
        let export = match statement {
            Statement::ImportDeclaration(_) => continue,
            Statement::ExportDefaultDeclaration(export) => export,
            other => return Err(reader.file.refuse(other.span(), expected)),
        };
        let Some(Expression::CallExpression(call)) = export
            .declaration
            .as_expression()
            .map(Expression::without_parentheses)
        else {
            return Err(reader.file.refuse(export.span, expected));
        };
        let declarer = match reader.helper(call) {
            Some(name @ ("page" | "layout" | "snippet" | "pageTemplate" | "buildingBlock"))
                if found.is_none() =>
            {
                name.to_string()
            }
            _ => return Err(reader.file.refuse(call.span, expected)),
        };
        let [module, element] = call.arguments.as_slice() else {
            return Err(reader.file.refuse(call.span, expected));
        };
        let (Some(Expression::StringLiteral(module)), Some(Expression::JSXElement(element))) =
            (argument_expression(module), argument_expression(element))
        else {
            return Err(reader.file.refuse(call.span, expected));
        };
        let mut document = reader.element(element, "")?;
        reader.resolve(&mut document)?;
        if super::declarer(&document.ty) != Some(declarer.as_str()) {
            return Err(reader.file.shape(
                element.span,
                format!("`{declarer}(...)` does not declare a {}", document.ty),
            ));
        }
        if !document.text("Name").is_some_and(is_name) {
            return Err(reader.file.shape(
                element.span,
                "its `name` is the name the model knows it by: letters, digits and `_`",
            ));
        }
        if !is_name(module.value.as_str()) {
            return Err(reader
                .file
                .shape(module.span, "a module's name: letters, digits and `_`"));
        }
        found = Some((module.value.to_string(), FormDecl { document }));
    }
    found.ok_or_else(|| FrontendError::Syntax {
        path: path.to_string(),
        detail: format!("it declares nothing: {expected}"),
    })
}

fn argument_expression<'p, 'a>(argument: &'p Argument<'a>) -> Option<&'p Expression<'a>> {
    argument
        .as_expression()
        .map(Expression::without_parentheses)
}
