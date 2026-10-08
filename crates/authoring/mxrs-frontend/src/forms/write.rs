//! Documents written as the TSX that declares them.

use std::collections::{BTreeSet, HashMap};

use mxrs_ir::{NativeDocument, NativeValue};

use super::tsx::{self, Attr, Element, Js};
use super::{
    CUSTOM_WIDGET, ELEMENTS_MODULE, FORMS_MODULE, Field, FieldDefault, Shape, TEXT, TRANSLATION,
    Vocabulary, WIDGET_OBJECT, WIDGET_PROPERTY, WIDGET_VALUE, WIDGETS_FOLDER, WidgetDefinition,
    join,
};

type Outcome<T> = Result<T, String>;

/// What an element states: its props, and its children.
type Stated = (Vec<(String, Attr)>, Vec<Element>);

struct Writer<'a> {
    vocabulary: &'a Vocabulary,
    root: &'a NativeDocument,
    /// How many documents of the root carry each name.
    names: HashMap<&'a str, usize>,
    elements: BTreeSet<String>,
    widgets: BTreeSet<String>,
    helpers: BTreeSet<&'static str>,
    /// The files beside the page its binary data is: the name each is
    /// imported by, its file's name and its bytes.
    files: Vec<(String, String, Vec<u8>)>,
}

/// The value element `shape` as a property of type `value_type` holds it
/// before a page says anything: what the widget's definition says its uses
/// hold (`defined`), when that is stored this way, and otherwise the
/// element's own defaults with the default the property's type declares.
/// Either way it points at its type.
pub(crate) fn property_default(
    shapes: &super::Shapes,
    shape: &Shape,
    value_type: &NativeDocument,
    value_type_path: &str,
    defined: Option<&NativeDocument>,
) -> Option<NativeDocument> {
    let mut base = match defined {
        Some(defined) if shape.fits(defined) => defined.clone(),
        _ => {
            let mut base = shapes.default_for(shape).clone();
            super::declare_default(&mut base, value_type);
            base
        }
    };
    base.get("TypePointer")?;
    base.set(
        "TypePointer",
        NativeValue::Pointer(value_type_path.to_string()),
    );
    Some(base)
}

/// The element a property's value is when a use of the widget does not
/// state it: the one the definition's default is, or the plain one.
pub(crate) fn unstated_shape<'s>(
    shapes: &'s super::Shapes,
    defined: Option<&NativeDocument>,
) -> Option<&'s Shape> {
    defined
        .and_then(|defined| shapes.shape_of(defined))
        .or_else(|| shapes.shape(WIDGET_VALUE))
}

/// `prefix` continued by a property's key.
pub(crate) fn keys(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        key.to_string()
    } else {
        format!("{prefix}.{key}")
    }
}

impl<'a> Writer<'a> {
    fn new(vocabulary: &'a Vocabulary, root: &'a NativeDocument) -> Self {
        let mut names: HashMap<&'a str, usize> = HashMap::new();
        super::walk(root, "", &mut |_, document| {
            if let Some(name) = document.text("Name") {
                *names.entry(name).or_default() += 1;
            }
        });
        Self {
            vocabulary,
            root,
            names,
            elements: BTreeSet::new(),
            widgets: BTreeSet::new(),
            helpers: BTreeSet::new(),
            files: Vec::new(),
        }
    }

    fn shape(&self, ty: &str, path: &str) -> Outcome<&'a Shape> {
        self.vocabulary
            .shapes
            .shape(ty)
            .ok_or_else(|| format!("{ty} at `{path}` is not an element of the project"))
    }

    /// The element `document` is: the one of its type with its fields.
    fn shaped(&self, document: &NativeDocument, path: &str) -> Outcome<&'a Shape> {
        self.vocabulary.shapes.shape_of(document).ok_or_else(|| {
            format!(
                "{} at `{path}` has fields no element of the project has",
                document.ty
            )
        })
    }

    /// The element `document` is, which is the one its type is when
    /// nothing says which: what is read without a component of its own.
    fn plainly_shaped(&self, document: &NativeDocument, path: &str) -> Outcome<&'a Shape> {
        let shape = self.shaped(document, path)?;
        if self.shape(&document.ty, path)?.component != shape.component {
            return Err(format!(
                "{} at `{path}` is stored another way than most of its kind",
                document.ty
            ));
        }
        Ok(shape)
    }

    fn element(&mut self, document: &NativeDocument, path: &str) -> Outcome<Element> {
        if document.ty == CUSTOM_WIDGET {
            return self.widget(document, path);
        }
        let shape = self.shaped(document, path)?;
        let base = self.vocabulary.shapes.default_for(shape);
        let (attrs, children) = self.fields(document, shape, base, &[], path)?;
        self.elements.insert(shape.component.clone());
        Ok(Element {
            tag: shape.component.clone(),
            attrs,
            children,
        })
    }

    /// What `document` states beyond `base`: its props, and its children.
    fn fields(
        &mut self,
        document: &NativeDocument,
        shape: &Shape,
        base: &NativeDocument,
        skip: &[&str],
        path: &str,
    ) -> Outcome<Stated> {
        let mut attrs = Vec::new();
        let mut children = Vec::new();
        for (field, (key, value)) in shape.fields.iter().zip(&document.fields) {
            if skip.contains(&key.as_str()) {
                continue;
            }
            let nested = join(path, key);
            if shape.children.as_deref() == Some(key) {
                let (NativeValue::List(marker, items), FieldDefault::List(expected)) =
                    (value, &field.default)
                else {
                    return Err(format!("`{nested}` is not the list its element holds"));
                };
                if marker != expected {
                    return Err(format!("`{nested}` is a list of another kind"));
                }
                for (index, item) in items.iter().enumerate() {
                    let NativeValue::Document(item) = item else {
                        return Err(format!("`{nested}` holds something that is no element"));
                    };
                    children.push(self.element(item, &format!("{nested}[{index}]"))?);
                }
                continue;
            }
            if base.get(key) == Some(value) {
                continue;
            }
            let attr = self.stated(value, field, base.get(key), &nested)?;
            attrs.push((field.prop.clone(), attr));
        }
        // What an element is called reads first.
        if let Some(name) = attrs.iter().position(|(prop, _)| prop == "name") {
            let name = attrs.remove(name);
            attrs.insert(0, name);
        }
        Ok((attrs, children))
    }

    /// What a prop is written as for `value`, where it would hold `base`
    /// were nothing said. An element that differs from the one already
    /// there in its main field alone is that field's value.
    fn stated(
        &mut self,
        value: &NativeValue,
        field: &Field,
        base: Option<&NativeValue>,
        path: &str,
    ) -> Outcome<Attr> {
        let shapes = &self.vocabulary.shapes;
        if let (
            NativeValue::Document(said),
            Some((shape, main)),
            Some(NativeValue::Document(held)),
        ) = (value, shapes.main_of(base), base)
        {
            let only_main = shape.fits(said)
                && said
                    .fields
                    .iter()
                    .zip(&held.fields)
                    .all(|((key, said), (_, held))| key == &main.key || said == held);
            let inner = said.get(&main.key);
            if let Some(inner) = inner
                && only_main
                && !matches!(inner, NativeValue::Null)
            {
                // What the field is written as must not itself be an
                // element: an element here reads as the whole.
                let used = (
                    self.elements.clone(),
                    self.widgets.clone(),
                    self.helpers.clone(),
                );
                let written =
                    self.stated(inner, main, held.get(&main.key), &join(path, &main.key))?;
                if !matches!(written, Attr::Value(Js::Element(_))) {
                    return Ok(written);
                }
                (self.elements, self.widgets, self.helpers) = used;
            }
            // Anything else is the element in full: what is not an element
            // here reads as its main field.
            return Ok(Attr::Value(Js::Element(self.element(said, path)?)));
        }
        Ok(match value {
            NativeValue::Text(text) => tsx::text_attr(text),
            // Texts by language are read as a text only where one belongs.
            NativeValue::Document(document)
                if document.ty == TEXT && !super::texts_fit(shapes, field, base) =>
            {
                Attr::Value(Js::Element(self.element(document, path)?))
            }
            other => Attr::Value(self.expression(other, Some(field), path)?),
        })
    }

    fn expression(
        &mut self,
        value: &NativeValue,
        field: Option<&Field>,
        path: &str,
    ) -> Outcome<Js> {
        // A number is as wide as its field's default, unless it says so.
        let wide = matches!(
            field.map(|field| &field.default),
            Some(FieldDefault::Value(NativeValue::Int64(_)))
        );
        Ok(match value {
            NativeValue::Null | NativeValue::Bool(_) | NativeValue::Text(_) => {
                Js::Raw(tsx::scalar(value).expect("a scalar"))
            }
            NativeValue::Int32(number) if wide => {
                self.helpers.insert("int");
                Js::Raw(format!("int({number})"))
            }
            NativeValue::Int32(number) => Js::Raw(number.to_string()),
            NativeValue::Int64(number) if wide => Js::Raw(number.to_string()),
            NativeValue::Int64(number) => {
                self.helpers.insert("long");
                Js::Raw(format!("long({number})"))
            }
            NativeValue::Document(document) => match self.texts(document) {
                Some(texts) => texts,
                None => Js::Element(self.element(document, path)?),
            },
            NativeValue::List(marker, items) => {
                match field.map(|field| &field.default) {
                    Some(FieldDefault::List(expected)) if expected == marker => {}
                    _ => return Err(format!("`{path}` is a list its element does not hold")),
                }
                let mut written = Vec::with_capacity(items.len());
                for (index, item) in items.iter().enumerate() {
                    written.push(self.expression(item, None, &format!("{path}[{index}]"))?);
                }
                Js::Array(written)
            }
            NativeValue::Pointer(target) => self.named(target, path)?,
            NativeValue::Identity(id) if id == NativeValue::NOTHING => {
                self.helpers.insert("unset");
                Js::Raw("unset".to_string())
            }
            NativeValue::Identity(id) => {
                self.helpers.insert("identity");
                Js::Raw(format!("identity({})", tsx::json(id)))
            }
            NativeValue::Binary(bytes) if bytes.is_empty() => {
                self.helpers.insert("binary");
                Js::Raw("binary()".to_string())
            }
            NativeValue::Binary(bytes) => Js::Raw(self.file(bytes, path)?),
        })
    }

    /// The name a file beside the page holding `bytes` is imported by: the
    /// prop that holds them, the file named for the page.
    fn file(&mut self, bytes: &[u8], path: &str) -> Outcome<String> {
        let extension = image_extension(bytes)
            .ok_or_else(|| format!("`{path}` holds data that is no image a page can import"))?;
        let key = path
            .rsplit('.')
            .next()
            .unwrap_or(path)
            .split('[')
            .next()
            .unwrap_or_default();
        let base = super::prop_of(key).unwrap_or_else(|| "data".to_string());
        let taken = |name: &str| self.files.iter().any(|(local, _, _)| local == name);
        let mut local = base.clone();
        let mut suffix = 2;
        while taken(&local) {
            local = format!("{base}{suffix}");
            suffix += 1;
        }
        let form = self.root.text("Name").unwrap_or("data");
        let file = match self.files.len() {
            0 => format!("{form}.{extension}"),
            // A dot no form's name holds keeps the next apart from them.
            count => format!("{form}.{}.{extension}", count + 1),
        };
        self.files.push((local.clone(), file, bytes.to_vec()));
        Ok(local)
    }

    /// A text as its translations by language code, when it is stored the
    /// one way that states.
    fn texts(&self, document: &NativeDocument) -> Option<Js> {
        let marker = self.vocabulary.shapes.text_marker()?;
        if document.ty != TEXT {
            return None;
        }
        let [(key, NativeValue::List(found, items))] = document.fields.as_slice() else {
            return None;
        };
        if key != "Items" || *found != marker {
            return None;
        }
        let mut entries: Vec<(String, Js)> = Vec::with_capacity(items.len());
        for item in items {
            let NativeValue::Document(translation) = item else {
                return None;
            };
            let [
                (language_key, NativeValue::Text(language)),
                (text_key, NativeValue::Text(text)),
            ] = translation.fields.as_slice()
            else {
                return None;
            };
            if translation.ty != TRANSLATION
                || language_key != "LanguageCode"
                || text_key != "Text"
                || entries.iter().any(|(known, _)| known == language)
            {
                return None;
            }
            entries.push((language.clone(), Js::Raw(tsx::text(text))));
        }
        Some(Js::Object(entries))
    }

    /// The document a pointer names, by the name only it carries.
    fn named(&mut self, target: &str, path: &str) -> Outcome<Js> {
        let name = self
            .root
            .at(target)
            .and_then(|document| document.text("Name"))
            .filter(|name| !name.is_empty() && self.names.get(name) == Some(&1))
            .ok_or_else(|| format!("`{path}` points at a document no name tells apart"))?;
        self.helpers.insert("named");
        Ok(Js::Raw(format!("named({})", tsx::json(name))))
    }

    fn widget(&mut self, document: &NativeDocument, path: &str) -> Outcome<Element> {
        let shape = self.plainly_shaped(document, path)?;
        let vocabulary = self.vocabulary;
        let (Some(NativeValue::Document(ty)), Some(NativeValue::Document(object))) =
            (document.get("Type"), document.get("Object"))
        else {
            return Err(format!(
                "the widget at `{path}` has no definition or no properties"
            ));
        };
        let definition = vocabulary
            .widget_of(ty)
            .ok_or_else(|| format!("the widget at `{path}` has a definition nothing declares"))?;
        let Some(NativeValue::Document(object_type)) = definition.ty.get("ObjectType") else {
            return Err(format!(
                "the widget at `{path}` has no properties to define"
            ));
        };
        if shape.field_of_prop("properties").is_some() {
            return Err("a widget has a field of its own called properties".to_string());
        }
        let properties = self.object(
            object,
            object_type,
            &format!("{}.ObjectType", join(path, "Type")),
            &join(path, "Object"),
            definition,
            "",
        )?;
        let base = vocabulary
            .shapes
            .default_document(CUSTOM_WIDGET)
            .expect("a shape has a default");
        let (mut attrs, children) =
            self.fields(document, shape, base, &["Type", "Object"], path)?;
        if !properties.is_empty() {
            attrs.push((
                "properties".to_string(),
                Attr::Value(Js::Object(properties)),
            ));
        }
        self.widgets.insert(definition.name.clone());
        Ok(Element {
            tag: definition.name.clone(),
            attrs,
            children,
        })
    }

    /// The properties `object` states, by their keys, where its type is
    /// `object_type` at `type_path`.
    fn object(
        &mut self,
        object: &NativeDocument,
        object_type: &NativeDocument,
        type_path: &str,
        path: &str,
        definition: &WidgetDefinition,
        prefix: &str,
    ) -> Outcome<Vec<(String, Js)>> {
        let vocabulary = self.vocabulary;
        let unreadable_for = |what: &str| {
            format!("the widget object at `{path}` is not stored as one is read: {what}")
        };
        let unreadable = || unreadable_for("its shape");
        if object.ty != WIDGET_OBJECT {
            return Err(unreadable());
        }
        let object_shape = self.plainly_shaped(object, path)?;
        let object_base = vocabulary
            .shapes
            .default_document(WIDGET_OBJECT)
            .expect("a shape has a default");
        let property_base = vocabulary
            .shapes
            .default_document(WIDGET_PROPERTY)
            .ok_or_else(unreadable)?;
        let mut properties: &[NativeValue] = &[];
        for (field, (key, value)) in object_shape.fields.iter().zip(&object.fields) {
            match (key.as_str(), value, &field.default) {
                ("TypePointer", NativeValue::Pointer(target), _) if target == type_path => {}
                ("Properties", NativeValue::List(marker, items), FieldDefault::List(expected))
                    if marker == expected =>
                {
                    properties = items;
                }
                ("TypePointer", ..) => return Err(unreadable_for("it points at another type")),
                ("Properties", ..) => return Err(unreadable_for("its list of properties")),
                _ if object_base.get(key) == Some(value) => {}
                _ => return Err(unreadable_for(key)),
            }
        }
        let types: &[NativeValue] = match object_type.get("PropertyTypes") {
            Some(NativeValue::List(_, types)) => types,
            _ => &[],
        };
        let mut entries: Vec<(String, Js)> = Vec::new();
        // The properties an object stores are its definition's, in order; a
        // definition that grew since may have some the object does not.
        let mut stored = properties.iter().enumerate().peekable();
        for (type_index, property_type) in types.iter().enumerate() {
            let property_type_path = format!("{type_path}.PropertyTypes[{type_index}]");
            let NativeValue::Document(property_type) = property_type else {
                return Err(unreadable());
            };
            let is_next = matches!(
                stored.peek(),
                Some((_, NativeValue::Document(property)))
                    if property.get("TypePointer")
                        == Some(&NativeValue::Pointer(property_type_path.clone()))
            );
            if !is_next {
                let key = property_type
                    .text("PropertyKey")
                    .filter(|key| entries.iter().all(|(known, _)| known != key))
                    .ok_or_else(unreadable)?;
                self.helpers.insert("missing");
                entries.push((key.to_string(), Js::Raw("missing".to_string())));
                continue;
            }
            let Some((index, NativeValue::Document(property))) = stored.next() else {
                return Err(unreadable());
            };
            let property_path = format!("{path}.Properties[{index}]");
            let Some(NativeValue::Document(value_type)) = property_type.get("ValueType") else {
                return Err(unreadable());
            };
            let key = property_type
                .text("PropertyKey")
                .filter(|key| entries.iter().all(|(known, _)| known != key))
                .ok_or_else(unreadable)?;
            if property.ty != WIDGET_PROPERTY || property.fields.len() != property_base.fields.len()
            {
                return Err(unreadable());
            }
            let mut value = None;
            for ((name, found), (expected_name, expected)) in
                property.fields.iter().zip(&property_base.fields)
            {
                match (name.as_str(), found) {
                    _ if name != expected_name => return Err(unreadable()),
                    ("TypePointer", NativeValue::Pointer(target))
                        if target == &property_type_path => {}
                    ("Value", NativeValue::Document(found)) if found.ty == WIDGET_VALUE => {
                        value = Some(found);
                    }
                    ("TypePointer" | "Value", _) => return Err(unreadable_for("a property")),
                    _ if found == expected => {}
                    _ => return Err(unreadable_for(name)),
                }
            }
            let value = value.ok_or_else(unreadable)?;
            let value_path = format!("{property_path}.Value");
            let value_type_path = format!("{property_type_path}.ValueType");
            // A value stored another way than most says so by its
            // component, and is then always stated.
            let value_shape = self.shaped(value, &value_path)?;
            let defined = definition.defaults.get(&keys(prefix, key));
            let unstated = unstated_shape(&vocabulary.shapes, defined)
                .and_then(|shape| {
                    property_default(
                        &vocabulary.shapes,
                        shape,
                        value_type,
                        &value_type_path,
                        defined,
                    )
                })
                .ok_or_else(unreadable)?;
            if value == &unstated {
                continue;
            }
            let base = property_default(
                &vocabulary.shapes,
                value_shape,
                value_type,
                &value_type_path,
                defined,
            )
            .ok_or_else(unreadable)?;
            if value.get("TypePointer") != base.get("TypePointer") {
                return Err(unreadable_for("a value points at another type"));
            }
            // The objects a property holds are stated by their own
            // properties, in the type the property's type nests.
            let mut objects = None;
            if let Some(NativeValue::List(marker, items)) = value.get("Objects")
                && base.get("Objects") != value.get("Objects")
            {
                let Some(NativeValue::Document(nested_type)) = value_type.get("ObjectType") else {
                    return Err(unreadable());
                };
                if base.get("Objects") != Some(&NativeValue::List(*marker, Vec::new())) {
                    return Err(unreadable());
                }
                let mut written = Vec::with_capacity(items.len());
                for (item_index, item) in items.iter().enumerate() {
                    let NativeValue::Document(item) = item else {
                        return Err(unreadable());
                    };
                    written.push(Js::Object(self.object(
                        item,
                        nested_type,
                        &format!("{value_type_path}.ObjectType"),
                        &format!("{value_path}.Objects[{item_index}]"),
                        definition,
                        &keys(prefix, key),
                    )?));
                }
                objects = Some(Js::Array(written));
            }
            // A property whose value says one thing, the thing a property
            // of its type is for, is written as that alone.
            let main = super::property_main(value_type)
                .and_then(|key| value_shape.field(key))
                .filter(|main| {
                    value_shape.fits(&unstated)
                        && value
                            .fields
                            .iter()
                            .zip(&unstated.fields)
                            .all(|((key, said), (_, held))| key == &main.key || said == held)
                        && !matches!(value.get(&main.key), Some(NativeValue::Null) | None)
                });
            if let Some(main) = main {
                let said = value.get(&main.key).expect("checked above");
                let written = match (&objects, said) {
                    (Some(objects), _) if main.key == "Objects" => objects.clone(),
                    (None, _) if main.key == "Objects" => return Err(unreadable()),
                    // The widgets a property holds: one alone, or a list.
                    (_, NativeValue::List(marker, items)) if main.key == "Widgets" => {
                        if unstated.get("Widgets") != Some(&NativeValue::List(*marker, Vec::new()))
                        {
                            return Err(unreadable());
                        }
                        let mut widgets = Vec::with_capacity(items.len());
                        for (index, item) in items.iter().enumerate() {
                            let NativeValue::Document(item) = item else {
                                return Err(unreadable());
                            };
                            widgets.push(Js::Element(
                                self.element(item, &format!("{value_path}.Widgets[{index}]"))?,
                            ));
                        }
                        match widgets.len() {
                            1 => widgets.remove(0),
                            _ => Js::Array(widgets),
                        }
                    }
                    // A flag reads as one.
                    (_, NativeValue::Text(flag))
                        if value_type.text("Type") == Some("Boolean")
                            && matches!(flag.as_str(), "true" | "false") =>
                    {
                        Js::Raw(flag.clone())
                    }
                    _ => {
                        if super::property_kind(&main.key, said).is_some() {
                            return Err(unreadable_for("a property holds what its type does not"));
                        }
                        let held = super::property_held(
                            &vocabulary.shapes,
                            &main.key,
                            unstated.get(&main.key),
                        );
                        match self.stated(
                            said,
                            main,
                            held.as_ref(),
                            &join(&value_path, &main.key),
                        )? {
                            Attr::Text(text) => Js::Raw(tsx::text(&text)),
                            Attr::Value(js) => js,
                        }
                    }
                };
                entries.push((key.to_string(), written));
                continue;
            }
            let (mut attrs, children) = self.fields(
                value,
                value_shape,
                &base,
                &["TypePointer", "Objects"],
                &value_path,
            )?;
            if let Some(objects) = objects {
                let prop = value_shape
                    .field("Objects")
                    .map(|field| field.prop.clone())
                    .ok_or_else(unreadable)?;
                attrs.push((prop, Attr::Value(objects)));
            }
            self.elements.insert(value_shape.component.clone());
            entries.push((
                key.to_string(),
                Js::Element(Element {
                    tag: value_shape.component.clone(),
                    attrs,
                    children,
                }),
            ));
        }
        if stored.next().is_some() {
            return Err(unreadable_for(
                "it states properties its definition does not have, or in another order",
            ));
        }
        Ok(entries)
    }

    fn imports(&self, declarer: &str) -> String {
        let mut out = String::new();
        let mut import = |names: Vec<String>, module: &str| {
            if names.is_empty() {
                return;
            }
            let line = format!("import {{ {} }} from \"{module}\";\n", names.join(", "));
            if line.len() <= 101 {
                out.push_str(&line);
            } else {
                out.push_str("import {\n");
                for name in names {
                    out.push_str(&format!("  {name},\n"));
                }
                out.push_str(&format!("}} from \"{module}\";\n"));
            }
        };
        import(self.elements.iter().cloned().collect(), ELEMENTS_MODULE);
        let mut helpers: BTreeSet<&str> = self.helpers.clone();
        helpers.insert(declarer);
        import(
            helpers.into_iter().map(str::to_string).collect(),
            FORMS_MODULE,
        );
        for widget in &self.widgets {
            import(
                vec![widget.clone()],
                &format!("@/{WIDGETS_FOLDER}/{widget}"),
            );
        }
        for (local, file, _) in &self.files {
            out.push_str(&format!("import {local} from \"./{file}\";\n"));
        }
        out
    }
}

/// The file that declares `document` — a page, layout or snippet of
/// `module` — or why nothing here can state it. A document holding binary
/// data needs files beside it: [`render_form_files`].
pub fn render_form(
    module: &str,
    document: &NativeDocument,
    vocabulary: &Vocabulary,
) -> Result<String, String> {
    let (source, files) = render_form_files(module, document, vocabulary)?;
    if !files.is_empty() {
        return Err("its binary data is files beside it".to_string());
    }
    Ok(source)
}

/// A form's file and the files beside it: its source, and each image's
/// file name and bytes.
pub type RenderedForm = (String, Vec<(String, Vec<u8>)>);

/// The file that declares `document` — a page, layout, snippet, page
/// template or building block of `module` — and the files beside it its
/// binary data is, by name; or why nothing here can state it.
pub fn render_form_files(
    module: &str,
    document: &NativeDocument,
    vocabulary: &Vocabulary,
) -> Result<RenderedForm, String> {
    let declarer = super::declarer(&document.ty)
        .ok_or_else(|| format!("{} is no form the frontend declares", document.ty))?;
    let mut writer = Writer::new(vocabulary, document);
    let element = writer.element(document, "")?;
    let mut body = tsx::lines(&Js::Element(element), 2);
    body.last_mut().expect("an element has a line").push(',');
    let source = format!(
        "{}\nexport default {declarer}(\n  {},\n{}\n);\n",
        writer.imports(declarer),
        tsx::json(module),
        body.join("\n")
    );
    let files = writer
        .files
        .into_iter()
        .map(|(_, file, bytes)| (file, bytes))
        .collect();
    Ok((source, files))
}

/// The extension of an image file holding `bytes`, by what they open
/// with; `None` for data that is no image a page can import.
fn image_extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("jpg")
    } else if bytes.starts_with(b"GIF8") {
        Some("gif")
    } else if bytes.len() > 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        // Markup opening an `<svg>` near its start, whatever comes first —
        // a byte order mark, an XML declaration, a comment, a doctype.
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]);
        let text = head.trim_start_matches('\u{feff}').trim_start();
        (text.starts_with('<') && text.contains("<svg")).then_some("svg")
    }
}

/// The file that declares a pluggable widget's definition.
pub fn render_widget(
    definition: &WidgetDefinition,
    vocabulary: &Vocabulary,
) -> Result<String, String> {
    let mut writer = Writer::new(vocabulary, &definition.ty);
    let element = writer.element(&definition.ty, "")?;
    let mut body = tsx::lines(&Js::Element(element), 2);
    body.last_mut().expect("an element has a line").push(',');
    if !definition.defaults.is_empty() {
        // What its properties hold before a page says anything.
        let mut defaults = Vec::with_capacity(definition.defaults.len());
        for (key, value) in &definition.defaults {
            let shape = writer.shaped(value, key)?;
            if shape.ty != WIDGET_VALUE {
                return Err(format!("the default of `{key}` is not a property's value"));
            }
            let base = vocabulary.shapes.default_for(shape);
            let (attrs, children) = writer.fields(value, shape, base, &["TypePointer"], key)?;
            writer.elements.insert(shape.component.clone());
            defaults.push((
                key.clone(),
                Js::Element(Element {
                    tag: shape.component.clone(),
                    attrs,
                    children,
                }),
            ));
        }
        body.extend(tsx::lines(&Js::Object(defaults), 2));
        body.last_mut().expect("an object has a line").push(',');
    }
    Ok(format!(
        "{}\nexport const {} = widget(\n{}\n);\n",
        writer.imports("widget"),
        definition.name,
        body.join("\n")
    ))
}
