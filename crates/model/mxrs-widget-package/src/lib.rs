//! Reads pluggable-widget metadata directly from installed MPK files and
//! builds the typed [`mxrs_pluggable::WidgetType`] schema without Studio
//! Pro. Ports `Mxrb::WidgetPackage` (`lib/mxrb/widget_package.rb`) — the
//! read/`find` side that turns a widget package's XML definition into the
//! embedded `CustomWidgets` schema. The write side (mxrb's hand-rolled
//! `template`) is not duplicated here: encoding goes through
//! `mxrs_pluggable::encode_widget`, the same complete codec real projects
//! round-trip through.
//!
//! One deliberate divergence: mxrb maps an XML property type it does not
//! recognize to a `nil` stored `Type`. That is silent corruption by this
//! project's standard, so an unrecognized property type is a loud
//! [`WidgetPackageError::UnknownPropertyType`] instead.

use std::path::{Path, PathBuf};

use mxrs_pluggable::{
    ActionVariable, EnumerationValue, ObjectType, PropertyType, ReturnType, Translation, ValueType,
    WidgetType,
};

/// `Mxrb::WidgetPackage::TYPE_MAP` — lowercase XML `type` attribute to the
/// canonical `CustomWidgets$WidgetValueType` kind.
const TYPE_MAP: [(&str, &str); 17] = [
    ("attribute", "Attribute"),
    ("expression", "Expression"),
    ("texttemplate", "TextTemplate"),
    ("widgets", "Widgets"),
    ("enumeration", "Enumeration"),
    ("boolean", "Boolean"),
    ("integer", "Integer"),
    ("datasource", "DataSource"),
    ("action", "Action"),
    ("selection", "Selection"),
    ("association", "Association"),
    ("object", "Object"),
    ("string", "String"),
    ("decimal", "Decimal"),
    ("icon", "Icon"),
    ("image", "Image"),
    ("file", "File"),
];

#[derive(Debug, thiserror::Error)]
pub enum WidgetPackageError {
    #[error("cannot read widget package {package}: {message}")]
    Package { package: String, message: String },
    #[error("widget definition in {package} is not valid XML: {message}")]
    Xml { package: String, message: String },
    #[error("widget property {key:?} in {package} has unrecognized type {type_name:?}")]
    UnknownPropertyType {
        package: String,
        key: String,
        type_name: String,
    },
}

pub type Result<T> = std::result::Result<T, WidgetPackageError>;

/// One installed `.mpk` widget package.
pub struct WidgetPackage {
    path: PathBuf,
}

impl WidgetPackage {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The typed schema of the widget with this id, or `None` when no
    /// widget definition in the package declares it. Mirrors
    /// `Mxrb::WidgetPackage#definition`.
    pub fn definition(&self, widget_id: &str) -> Result<Option<WidgetType>> {
        let package = self.path.display().to_string();
        let file =
            std::fs::File::open(&self.path).map_err(|error| WidgetPackageError::Package {
                package: package.clone(),
                message: error.to_string(),
            })?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|error| WidgetPackageError::Package {
                package: package.clone(),
                message: error.to_string(),
            })?;
        for name in widget_entry_names(&mut archive, &package)? {
            let source = read_entry(&mut archive, &name, &package)?;
            // A malformed widget-definition entry fails the whole package
            // (mxrb raises REXML::ParseException here too); `find` treats
            // that package as unreadable and moves on.
            let root = parse_xml(&source).map_err(|message| WidgetPackageError::Xml {
                package: package.clone(),
                message,
            })?;
            if attribute(&root, "id") == Some(widget_id) {
                return build_definition(&root, &package).map(Some);
            }
        }
        Ok(None)
    }
}

/// First matching definition across every `widgets/*.mpk` under `root`, in
/// sorted path order — mirrors `Mxrb::WidgetPackage.find`, including
/// skipping unreadable packages rather than failing the lookup.
pub fn find(root: impl AsRef<Path>, widget_id: &str) -> Option<WidgetType> {
    for path in mpk_paths(&root.as_ref().join("widgets")) {
        if let Ok(Some(definition)) = WidgetPackage::new(path).definition(widget_id) {
            return Some(definition);
        }
    }
    None
}

fn mpk_paths(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("mpk"))
        })
        .collect();
    paths.sort();
    paths
}

/// The widget-definition XML entries: the ones `package.xml` names via
/// `widgetFile` paths, or every `.xml` entry when there is no manifest.
fn widget_entry_names(
    archive: &mut zip::ZipArchive<std::fs::File>,
    package: &str,
) -> Result<Vec<String>> {
    let names: Vec<String> = archive.file_names().map(str::to_string).collect();
    let manifest = names.iter().find(|name| name.as_str() == "package.xml");
    let Some(manifest) = manifest.cloned() else {
        let mut xml: Vec<String> = names
            .into_iter()
            .filter(|name| name.to_lowercase().ends_with(".xml"))
            .collect();
        xml.sort();
        return Ok(xml);
    };
    let source = read_entry(archive, &manifest, package)?;
    let root = parse_xml(&source).map_err(|message| WidgetPackageError::Xml {
        package: package.to_string(),
        message,
    })?;
    let mut paths = Vec::new();
    collect_recursive(&root, "widgetFile", &mut |element| {
        if let Some(path) = attribute(element, "path") {
            paths.push(path.to_string());
        }
    });
    Ok(paths
        .into_iter()
        .filter(|path| archive.by_name(path).is_ok())
        .collect())
}

fn read_entry(
    archive: &mut zip::ZipArchive<std::fs::File>,
    name: &str,
    package: &str,
) -> Result<String> {
    use std::io::Read;
    let mut entry = archive
        .by_name(name)
        .map_err(|error| WidgetPackageError::Package {
            package: package.to_string(),
            message: error.to_string(),
        })?;
    let mut source = String::new();
    entry
        .read_to_string(&mut source)
        .map_err(|error| WidgetPackageError::Package {
            package: package.to_string(),
            message: error.to_string(),
        })?;
    Ok(source)
}

// --- Minimal namespace-agnostic XML DOM -----------------------------------
//
// mxrb reads these files through REXML by *local* element name, so
// namespace prefixes never matter; this mirrors that with quick-xml.

struct Element {
    name: String,
    attributes: Vec<(String, String)>,
    children: Vec<Element>,
    text: String,
}

fn parse_xml(source: &str) -> std::result::Result<Element, String> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(source);
    let mut stack: Vec<Element> = Vec::new();
    loop {
        match reader.read_event().map_err(|error| error.to_string())? {
            Event::Start(start) => stack.push(element_from(&start)?),
            Event::Empty(start) => {
                let element = element_from(&start)?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(element),
                    None => return Ok(element),
                }
            }
            Event::End(_) => {
                let element = stack.pop().ok_or("unbalanced closing tag")?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(element),
                    None => return Ok(element),
                }
            }
            Event::Text(text) => {
                if let Some(element) = stack.last_mut() {
                    element
                        .text
                        .push_str(&text.decode().map_err(|error| error.to_string())?);
                }
            }
            Event::CData(data) => {
                if let Some(element) = stack.last_mut() {
                    element.text.push_str(&String::from_utf8_lossy(&data));
                }
            }
            // quick-xml surfaces `&amp;`-style references as their own
            // event instead of expanding them inside `Event::Text`.
            Event::GeneralRef(reference) => {
                if let Some(element) = stack.last_mut() {
                    let name = String::from_utf8_lossy(&reference).to_string();
                    let resolved = match name.as_str() {
                        "amp" => Some('&'),
                        "lt" => Some('<'),
                        "gt" => Some('>'),
                        "quot" => Some('"'),
                        "apos" => Some('\''),
                        _ => reference
                            .resolve_char_ref()
                            .map_err(|error| error.to_string())?,
                    };
                    match resolved {
                        Some(character) => element.text.push(character),
                        None => return Err(format!("unresolvable entity reference: &{name};")),
                    }
                }
            }
            Event::Eof => return Err("unexpected end of document".to_string()),
            _ => {}
        }
    }
}

fn element_from(start: &quick_xml::events::BytesStart<'_>) -> std::result::Result<Element, String> {
    let name = local_name(&String::from_utf8_lossy(start.name().as_ref()));
    let mut attributes = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|error| error.to_string())?;
        let key = String::from_utf8_lossy(attribute.key.as_ref()).to_string();
        if key == "xmlns" || key.starts_with("xmlns:") {
            continue;
        }
        let value = attribute
            .unescape_value()
            .map_err(|error| error.to_string())?
            .into_owned();
        attributes.push((local_name(&key), value));
    }
    Ok(Element {
        name,
        attributes,
        children: Vec::new(),
        text: String::new(),
    })
}

fn local_name(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).to_string()
}

fn attribute<'element>(element: &'element Element, name: &str) -> Option<&'element str> {
    element
        .attributes
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn child<'element>(element: &'element Element, name: &str) -> Option<&'element Element> {
    element.children.iter().find(|child| child.name == name)
}

fn child_text(element: &Element, name: &str) -> String {
    child(element, name)
        .map(|child| child.text.clone())
        .unwrap_or_default()
}

fn collect_recursive<'element>(
    element: &'element Element,
    name: &str,
    found: &mut impl FnMut(&'element Element),
) {
    for candidate in &element.children {
        if candidate.name == name {
            found(candidate);
        }
        collect_recursive(candidate, name, found);
    }
}

fn nested_elements<'element>(
    element: &'element Element,
    container: &str,
    name: &str,
) -> Vec<&'element Element> {
    child(element, container)
        .map(|wrapper| {
            wrapper
                .children
                .iter()
                .filter(|child| child.name == name)
                .collect()
        })
        .unwrap_or_default()
}

fn nested_element_names(element: &Element, container: &str, name: &str) -> Vec<String> {
    nested_elements(element, container, name)
        .into_iter()
        .filter_map(|value| attribute(value, "name").map(str::to_string))
        .collect()
}

// --- XML → typed schema ----------------------------------------------------

fn build_definition(root: &Element, package: &str) -> Result<WidgetType> {
    let properties = match child(root, "properties") {
        Some(container) => property_groups(container, None, package)?,
        None => Vec::new(),
    };
    let platform = attribute(root, "supportedPlatform").unwrap_or_default();
    Ok(WidgetType {
        id: attribute(root, "id").unwrap_or_default().to_string(),
        name: child_text(root, "name"),
        description: child_text(root, "description"),
        prompt: String::new(),
        studio_pro_category: child_text(root, "studioProCategory"),
        studio_category: child_text(root, "studioCategory"),
        platform: if platform.is_empty() {
            "Web".to_string()
        } else {
            platform.to_string()
        },
        offline: attribute(root, "offlineCapable") == Some("true"),
        needs_context: attribute(root, "needsEntityContext") == Some("true"),
        plugin: attribute(root, "pluginWidget") == Some("true"),
        help_url: child_text(root, "helpUrl"),
        object_type: ObjectType { properties },
    })
}

/// Flattens `propertyGroup` nesting the way mxrb does: a group contributes
/// its caption to the property category, joined with `::`.
fn property_groups(
    parent: &Element,
    category: Option<&str>,
    package: &str,
) -> Result<Vec<PropertyType>> {
    let mut properties = Vec::new();
    for element in &parent.children {
        match element.name.as_str() {
            "property" => {
                properties.push(property(element, effective_category(category), package)?);
            }
            "systemProperty" => {
                properties.push(system_property(element, effective_category(category)));
            }
            "propertyGroup" => {
                let caption = attribute(element, "caption").unwrap_or_default();
                let nested: Vec<&str> = [category.unwrap_or_default(), caption]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect();
                let nested = nested.join("::");
                properties.extend(property_groups(element, Some(&nested), package)?);
            }
            _ => {}
        }
    }
    Ok(properties)
}

fn effective_category(category: Option<&str>) -> &str {
    match category {
        Some(category) if !category.is_empty() => category,
        _ => "General",
    }
}

fn property(element: &Element, category: &str, package: &str) -> Result<PropertyType> {
    let key = attribute(element, "key").unwrap_or_default().to_string();
    let type_name = attribute(element, "type").unwrap_or_default();
    let kind = TYPE_MAP
        .iter()
        .find(|(xml, _)| *xml == type_name.to_lowercase())
        .map(|(_, kind)| (*kind).to_string())
        .ok_or_else(|| WidgetPackageError::UnknownPropertyType {
            package: package.to_string(),
            key: key.clone(),
            type_name: type_name.to_string(),
        })?;
    let children = match child(element, "properties") {
        Some(container) => property_groups(container, None, package)?
            .into_iter()
            .filter(|property| !property.is_system())
            .collect(),
        None => Vec::new(),
    };
    let default_type = attribute(element, "defaultType").unwrap_or_default();
    Ok(PropertyType {
        key,
        category: category.to_string(),
        caption: child_text(element, "caption"),
        description: child_text(element, "description"),
        prompt: String::new(),
        default: false,
        value_type: ValueType {
            kind,
            list: attribute(element, "isList") == Some("true"),
            linked: attribute(element, "isLinked") == Some("true"),
            metadata: attribute(element, "isMetaData") == Some("true"),
            entity_property: String::new(),
            allow_non_persistable_entities: false,
            path_kind: "No".to_string(),
            path_type: "None".to_string(),
            parameter_list: false,
            multiline: attribute(element, "multiline") == Some("true"),
            default_value: attribute(element, "defaultValue")
                .unwrap_or_default()
                .to_string(),
            required: attribute(element, "required") != Some("false"),
            on_change_property: attribute(element, "onChange")
                .unwrap_or_default()
                .to_string(),
            data_source_property: attribute(element, "dataSource")
                .unwrap_or_default()
                .to_string(),
            selectable_objects_property: attribute(element, "selectableObjects")
                .unwrap_or_default()
                .to_string(),
            attribute_types: nested_element_names(element, "attributeTypes", "attributeType"),
            association_types: nested_element_names(element, "associationTypes", "associationType"),
            selection_types: nested_element_names(element, "selectionTypes", "selectionType"),
            enumeration_values: nested_elements(element, "enumerationValues", "enumerationValue")
                .into_iter()
                .map(|value| EnumerationValue {
                    key: attribute(value, "key").unwrap_or_default().to_string(),
                    caption: value.text.trim().to_string(),
                })
                .collect(),
            action_variables: nested_elements(element, "actionVariables", "actionVariable")
                .into_iter()
                .map(|variable| ActionVariable {
                    key: attribute(variable, "key").unwrap_or_default().to_string(),
                    kind: attribute(variable, "type").unwrap_or_default().to_string(),
                    caption: attribute(variable, "caption")
                        .unwrap_or_default()
                        .to_string(),
                })
                .collect(),
            object_type: if children.is_empty() {
                None
            } else {
                Some(Box::new(ObjectType {
                    properties: children,
                }))
            },
            return_type: child(element, "returnType").map(|node| {
                let kind = attribute(node, "type").unwrap_or_default();
                ReturnType {
                    kind: if kind.is_empty() {
                        "None".to_string()
                    } else {
                        kind.to_string()
                    },
                    list: false,
                    entity_property: String::new(),
                    assignable_to: attribute(node, "assignableTo")
                        .unwrap_or_default()
                        .to_string(),
                }
            }),
            translations: nested_elements(element, "translations", "translation")
                .into_iter()
                .map(|translation| Translation {
                    language: attribute(translation, "lang")
                        .unwrap_or_default()
                        .to_string(),
                    text: translation.text.trim().to_string(),
                })
                .collect(),
            set_label: attribute(element, "setLabel") == Some("true"),
            default_type: if default_type.is_empty() {
                "None".to_string()
            } else {
                default_type.to_string()
            },
            allow_upload: false,
        },
    })
}

/// A `systemProperty` contributes a schema slot but never a stored value —
/// mirrors `Mxrb::WidgetPackage#system_property`, including the
/// `<system:key>` caption convention.
fn system_property(element: &Element, category: &str) -> PropertyType {
    let key = attribute(element, "key").unwrap_or_default().to_string();
    PropertyType {
        caption: format!("<system:{key}>"),
        key,
        category: category.to_string(),
        description: String::new(),
        prompt: String::new(),
        default: false,
        value_type: ValueType {
            kind: "System".to_string(),
            list: false,
            linked: false,
            metadata: false,
            entity_property: String::new(),
            allow_non_persistable_entities: false,
            path_kind: "No".to_string(),
            path_type: "None".to_string(),
            parameter_list: false,
            multiline: false,
            default_value: String::new(),
            required: false,
            on_change_property: String::new(),
            data_source_property: String::new(),
            selectable_objects_property: String::new(),
            attribute_types: Vec::new(),
            association_types: Vec::new(),
            selection_types: Vec::new(),
            enumeration_values: Vec::new(),
            action_variables: Vec::new(),
            object_type: None,
            return_type: None,
            translations: Vec::new(),
            set_label: false,
            default_type: "None".to_string(),
            allow_upload: false,
        },
    }
}
