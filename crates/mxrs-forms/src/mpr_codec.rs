//! Strict boundary between physical BSON documents and the typed
//! [`Node`] model. Unknown storage fields fail with their path; no native
//! fragment is kept.
//!
//! Ports the core (non-pluggable) path of `Mxrb::Forms::MprCodec` from
//! `lib/mxrb/forms/mpr_codec.rb`.
//!
//! **Important, confirmed by experiment while porting this**: mxrb has
//! *two independent* ways of producing a Forms page document, and this
//! codec is only the decoder for one of them.
//!
//! Path one is `dsl/builder.rb`'s widget DSL (`page`/`button`/`data_grid`/
//! `text`/...) plus `writer.rb` (7316 lines) — an older, hand-rolled,
//! per-widget writer that predates the schema-driven Forms system. `mxrb
//! generate` uses this path. Its output can include legacy/dual-encoded
//! shapes (e.g. a `Button` with *both* a flat `Caption` field and a
//! `CaptionTemplate`) that `Forms::MprCodec` — mxrb's own, not just mxrs's
//! port — does **not** decode. Confirmed directly: `mxrb export` on a
//! DSL-generated page still succeeds, but only because `exporter.rb`
//! explicitly rescues `Forms::MprCodecError` and falls back to a lossless
//! "semantic page baseline" projection instead, with a comment
//! acknowledging "transitional widgets with concise, semantic fields
//! outside their embedded schema."
//!
//! Path two is `Mxrb::Forms.build`/`Node` (the schema-driven API this crate
//! mirrors) plus `Forms::MprCodec.encode` — used by `writer.rb` only for
//! documents that already carry a `:forms_model` (a `Forms::Node` built via
//! this API, not the plain DSL), and by `exporter.rb` to decode existing
//! pages. This is the shape `Forms::MprCodec`/`mxrs-forms` actually
//! targets, confirmed by round-tripping a real `Forms::MprCodec`-encoded
//! document (see `tests/native_page.rs`) end to end.
//!
//! Practical consequence for later phases: a page authored purely through
//! `mxrs-dsl`'s eventual widget builder (mirroring path 2) round-trips
//! cleanly through this codec; a page imported from an arbitrary existing
//! `.mpr` may hit path 1's legacy shapes and need either a native-fragment
//! fallback (mirroring `native_fragment_store.rb`) or porting the specific
//! dual-field shims mxrb's writer.rb emits — not a defect in this port.
//!
//! **`CustomWidgets$CustomWidget` handling**: decoding now delegates to
//! `mxrs-pluggable` for real (`decode_one` returns `Value::Pluggable` —
//! see `decode_pluggable` below), matching `Mxrb::Forms::MprCodec
//! #decode_embedded`'s `pluggable_codec.decode` branch. `MprCodec` also
//! now implements `mxrs_pluggable::EmbeddedFormsDecoder` (see the
//! `EmbeddedDecoder` wrapper below `impl MprCodec`) — the dependency-
//! inversion seam that lets `mxrs-pluggable` decode a `TextTemplate`/
//! `Action`/`Icon` value's embedded Forms element without this crate
//! depending back on it (mirroring mxrb's `Pluggable::MprCodec`
//! constructor taking a `forms_codec:` callback). Still explicitly
//! erroring rather than silently dropping data: (a) the remaining
//! non-self-contained slices of `DataSource`/`Widgets` in
//! `mxrs-pluggable` (see that crate's module doc — mechanical follow-up,
//! same seam, not a new blocker), and (b) encoding a `Value::Pluggable`
//! back to BSON at all (`mxrs-pluggable::encode_object`/`encode_value`
//! aren't ported yet).
//!
//! Deliberately **not** ported:
//! - The `OBSOLETE_DEFAULT_FIELDS` shim and the legacy attribute-path/
//!   label-text/source-variable/design-property/placeholder branches, and
//!   the boolean-as-enum shim — these are for genuinely pre-11.x document
//!   shapes (distinct from the dual-field quirks above, which are current);
//!   out of scope per the locked 11.x-only MVP. The flat `Class`/`Style`
//!   appearance shim **is** ported despite initially looking like a
//!   same-era legacy case — it's confirmed present in fresh 11.12.1 output
//!   (e.g. `Forms$DynamicText`), not just old projects.
//!
//! `reference_decoder`/`reference_encoder` mirror mxrb's injectable
//! resolvers for by-id references that don't resolve to a name inside the
//! same document: mxrs-forms has no notion of a wider project, so
//! cross-document reference resolution is `mxrs-model`'s job (Phase 2's
//! third crate), injected here as plain closures.

use std::collections::HashMap;
use std::rc::Rc;

use mxrs_bson::{Bson, Document};

use crate::catalog::Size;
use crate::catalog::{Catalog, Property, ReferenceKind};
use crate::error::{FormsError, Result};
use crate::node::{Node, Value};
use crate::storage_naming;
use crate::values::{
    BinaryAsset, Condition, DataType, Reference, Text, TextTemplate, Translation, XPathConstraint,
};
use mxrs_forms_refs::{
    decode_attribute_reference, decode_entity_reference, encode_attribute_reference,
    encode_entity_reference,
};

const INTERNAL_PREFIXES: &[&str] = &["Forms$", "Pages$"];
const COMPANION_FIELDS: &[&str] = &["$ID", "$Type", "ExpressionModel"];

/// `((declared_by, property name), marker)`; anything absent defaults to
/// `2` (or `1` for a `by_name` reference property, checked first).
const COLLECTION_MARKERS: &[((&str, &str), i32)] = &[
    (("Appearance", "designProperties"), 3),
    (("ClientTemplate", "parameters"), 2),
    (("ConditionallyVisibleWidget", "moduleRoles"), 1),
    (("ControlBar", "items"), 3),
    (("DataView", "footerWidgets"), 2),
    (("DataView", "widgets"), 2),
    (("MicroflowSettings", "outputMappings"), 3),
    (("Page", "parameters"), 3),
    (("TabContainer", "tabPages"), 3),
    (("WidgetValidation", "conditions"), 2),
];

type ReferenceDecoder = dyn Fn(&Bson, &str) -> Result<String>;
type ReferenceEncoder = dyn Fn(&str, &str) -> Result<Bson>;

pub struct MprCodec {
    catalog: Rc<Catalog>,
    reference_decoder: Option<Box<ReferenceDecoder>>,
    reference_encoder: Option<Box<ReferenceEncoder>>,
}

impl MprCodec {
    pub fn new(catalog: Rc<Catalog>) -> Self {
        Self {
            catalog,
            reference_decoder: None,
            reference_encoder: None,
        }
    }

    pub fn with_reference_decoder(
        mut self,
        f: impl Fn(&Bson, &str) -> Result<String> + 'static,
    ) -> Self {
        self.reference_decoder = Some(Box::new(f));
        self
    }

    pub fn with_reference_encoder(
        mut self,
        f: impl Fn(&str, &str) -> Result<Bson> + 'static,
    ) -> Self {
        self.reference_encoder = Some(Box::new(f));
        self
    }

    pub fn decode(&self, document: &Document) -> Result<Node> {
        let local_references = storage_reference_names(document);
        self.decode_node(document, "$", &local_references)
    }

    pub fn encode(&self, node: &Node) -> Result<Document> {
        let local_references = self.collect_local_reference_names(node);
        self.encode_node(node, "$", &local_references)
    }

    // ── Decode ───────────────────────────────────────────────────────────

    fn decode_node(
        &self,
        document: &Document,
        path: &str,
        local_references: &HashMap<String, String>,
    ) -> Result<Node> {
        let type_field = document
            .get_str("$Type")
            .map_err(|_| FormsError::InvalidShape {
                shape: "Forms document ($Type missing)",
                path: path.to_string(),
            })?;
        if type_field == "CustomWidgets$CustomWidget" {
            // Never hit by real Mendix data (a page's root/element document
            // is never itself a `CustomWidgets$CustomWidget` — it's always
            // nested inside a widget-typed property, handled in
            // `decode_one` instead, which can return `Value::Pluggable`).
            // `Node` has no representation for pluggable content, so this
            // stays a defensive, explicit error rather than a silent
            // `todo!()` if that assumption ever turns out wrong.
            return Err(FormsError::InvalidShape {
                shape: "top-level CustomWidgets$CustomWidget (must be nested in a widget property)",
                path: path.to_string(),
            });
        }
        let type_name = self.internal_type_name(type_field)?;
        let schema_type = self.catalog.fetch_type(&type_name)?;

        let mut node = Node::new(schema_type.name.clone(), self.catalog.clone())?;
        let mut consumed: std::collections::HashSet<String> =
            COMPANION_FIELDS.iter().map(|s| (*s).to_string()).collect();

        for property in &schema_type.all_properties {
            // Some widgets still store `appearance` as flat `Class`/`Style`
            // fields on the widget itself rather than a nested `Appearance`
            // document — confirmed present even in freshly mxrb-generated
            // 11.12.1 content (e.g. `Forms$DynamicText`), not just documents
            // from an older Mendix version.
            if property.name == "appearance"
                && (document.contains_key("Class") || document.contains_key("Style"))
            {
                consumed.insert("Class".to_string());
                consumed.insert("Style".to_string());
                if !document.contains_key("Appearance") {
                    node.set(
                        &property.name,
                        Value::Node(self.decode_flat_appearance(document)?),
                    )?;
                    continue;
                }
            }

            let Some(storage) = storage_naming::candidates(property)
                .into_iter()
                .find(|c| document.contains_key(c))
            else {
                continue;
            };
            consumed.insert(storage.clone());
            let raw = document.get(&storage).expect("just checked contains_key");
            let value = self.decode_property(
                property,
                raw,
                &format!("{path}.{storage}"),
                local_references,
            )?;
            node.set(&property.name, value)?;
        }

        let mut unknown: Vec<&String> = document
            .keys()
            .filter(|k| !consumed.contains(k.as_str()))
            .collect();
        if !unknown.is_empty() {
            unknown.sort();
            let fields = unknown.into_iter().cloned().collect::<Vec<_>>().join(", ");
            return Err(FormsError::UnsupportedStorageProperty {
                type_name: schema_type.name.clone(),
                path: path.to_string(),
                fields,
            });
        }
        Ok(node)
    }

    /// Decodes a `CustomWidgets$CustomWidget` document (`Type` +
    /// `Object` sub-documents) via `mxrs-pluggable`, translating its
    /// `PluggableError` into `FormsError::PluggableDecodeFailed` — never a
    /// silent drop, matching every other codec boundary in this crate.
    /// Ports the `pluggable_codec.decode` branch of
    /// `Mxrb::Forms::MprCodec#decode_embedded`.
    fn decode_pluggable(
        &self,
        document: &Document,
        path: &str,
        local_references: &HashMap<String, String>,
    ) -> Result<mxrs_pluggable::ObjectNode<Node>> {
        let wrap = |source| FormsError::PluggableDecodeFailed {
            path: path.to_string(),
            source,
        };
        let type_path = format!("{path}.Type");
        let type_document =
            document
                .get_document("Type")
                .map_err(|_| FormsError::InvalidShape {
                    shape: "CustomWidgets$CustomWidget.Type",
                    path: path.to_string(),
                })?;
        let (_widget_type, context) =
            mxrs_pluggable::decode_widget_type(type_document, &type_path).map_err(wrap)?;
        let object_path = format!("{path}.Object");
        let object_document =
            document
                .get_document("Object")
                .map_err(|_| FormsError::InvalidShape {
                    shape: "CustomWidgets$CustomWidget.Object",
                    path: path.to_string(),
                })?;
        // Nested pluggable values share the root document's semantic
        // reference maps (same comment mxrb's own `decode_embedded` makes)
        // — this hook closes over the *caller's* `local_references` rather
        // than recomputing a fresh (and wrong: scoped only to the tiny
        // embedded fragment) map for whatever `TextTemplate`/`Action`/
        // `Icon` element ends up nested inside this widget's properties.
        let embedded = EmbeddedDecoder {
            codec: self,
            local_references,
        };
        mxrs_pluggable::decode_object(object_document, &context, &object_path, &embedded)
            .map_err(wrap)
    }

    fn decode_property(
        &self,
        property: &Property,
        raw: &Bson,
        path: &str,
        local_references: &HashMap<String, String>,
    ) -> Result<Value> {
        if matches!(raw, Bson::Null) && property.optional {
            return Ok(Value::Null);
        }
        if property.many() {
            let items = match raw {
                Bson::Array(items) => mxrs_bson::parse_array(Some(items)).items,
                _ => {
                    return Err(FormsError::ExpectedArray {
                        type_name: property.declared_by.clone(),
                        property: property.name.clone(),
                    });
                }
            };
            let decoded = items
                .iter()
                .enumerate()
                .map(|(i, item)| {
                    self.decode_one(property, item, &format!("{path}[{i}]"), local_references)
                })
                .collect::<Result<Vec<_>>>()?;
            return Ok(Value::List(decoded));
        }
        self.decode_one(property, raw, path, local_references)
    }

    fn decode_one(
        &self,
        property: &Property,
        raw: &Bson,
        path: &str,
        local_references: &HashMap<String, String>,
    ) -> Result<Value> {
        if property.is_reference() {
            return self.decode_reference(property, raw, path, local_references);
        }
        match property.type_name.as_str() {
            "blob" => return self.decode_blob(raw, path),
            "string" => return self.expect_string(raw, path).map(Value::String),
            "integer" => return self.expect_integer(raw, path).map(Value::Integer),
            "boolean" => return self.expect_boolean(raw, path).map(Value::Boolean),
            "size" => return self.decode_size(raw, path),
            _ => {}
        }
        if let Some(target) = self.catalog.type_(&property.type_name) {
            if target.is_enum() {
                return self.expect_string(raw, path).map(Value::String);
            }
            if target.is_element() {
                let Bson::Document(doc) = raw else {
                    return Err(FormsError::InvalidShape {
                        shape: "element",
                        path: path.to_string(),
                    });
                };
                if doc.get_str("$Type").ok() == Some("CustomWidgets$CustomWidget") {
                    return self
                        .decode_pluggable(doc, path, local_references)
                        .map(Value::Pluggable);
                }
                return Ok(Value::Node(self.decode_node(
                    doc,
                    path,
                    local_references,
                )?));
            }
        }
        self.decode_external(&property.type_name, raw, path)
    }

    fn decode_external(&self, type_name: &str, raw: &Bson, path: &str) -> Result<Value> {
        match type_name {
            "Text" => self.decode_text(raw, path).map(Value::Text),
            "Expression" => self
                .expect_string(raw, path)
                .map(|s| Value::Expression(crate::values::Expression::new(s))),
            "AttributeReference" => decode_attribute_reference(raw, path)
                .map(Value::AttributeReference)
                .map_err(Self::ref_error),
            "EntityReference" => decode_entity_reference(raw, path)
                .map(Value::EntityReference)
                .map_err(Self::ref_error),
            "DataType" => self.decode_data_type(raw, path).map(Value::DataType),
            "Condition" => self.decode_condition(raw, path).map(Value::Condition),
            "TextTemplate" => self
                .decode_text_template(raw, path)
                .map(Value::TextTemplate),
            "XPathConstraint" => self
                .decode_xpath_constraint(raw, path)
                .map(Value::XPathConstraint),
            other => Err(FormsError::UnsupportedExternalType {
                type_name: other.to_string(),
                path: path.to_string(),
            }),
        }
    }

    fn decode_reference(
        &self,
        property: &Property,
        raw: &Bson,
        path: &str,
        local_references: &HashMap<String, String>,
    ) -> Result<Value> {
        let kind = property
            .reference
            .expect("is_reference() checked by caller");
        if kind != ReferenceKind::ById {
            let target = self.expect_string(raw, path)?;
            return Ok(Value::Reference(Reference { target, kind }));
        }
        let identifier = mxrs_bson::extract_id(raw).unwrap_or_default();
        if identifier == "00000000-0000-0000-0000-000000000000" {
            return Ok(Value::Null);
        }
        let target = if let Some(local) = local_references.get(&identifier) {
            local.clone()
        } else if let Some(decoder) = &self.reference_decoder {
            decoder(raw, path)?
        } else {
            return Err(FormsError::UnresolvedStorageReference {
                path: path.to_string(),
            });
        };
        Ok(Value::Reference(Reference { target, kind }))
    }

    /// Synthesizes an `Appearance` node from a widget's flat `Class`/`Style`
    /// fields (see the call site in `decode_node`).
    fn decode_flat_appearance(&self, document: &Document) -> Result<Node> {
        let mut appearance = Node::new("Appearance", self.catalog.clone())?;
        appearance.set(
            "class",
            Value::String(document.get_str("Class").unwrap_or("").to_string()),
        )?;
        appearance.set(
            "style",
            Value::String(document.get_str("Style").unwrap_or("").to_string()),
        )?;
        appearance.set("designProperties", Value::List(Vec::new()))?;
        appearance.set("dynamicClasses", Value::String(String::new()))?;
        Ok(appearance)
    }

    fn decode_size(&self, raw: &Bson, path: &str) -> Result<Value> {
        let Bson::Document(doc) = raw else {
            return Err(FormsError::InvalidShape {
                shape: "size",
                path: path.to_string(),
            });
        };
        let width = doc
            .get_i32("Width")
            .or_else(|_| doc.get_i32("width"))
            .map_err(|_| FormsError::InvalidShape {
                shape: "size.width",
                path: path.to_string(),
            })?;
        let height = doc
            .get_i32("Height")
            .or_else(|_| doc.get_i32("height"))
            .map_err(|_| FormsError::InvalidShape {
                shape: "size.height",
                path: path.to_string(),
            })?;
        Ok(Value::Size(Size {
            width: i64::from(width),
            height: i64::from(height),
        }))
    }

    fn decode_blob(&self, raw: &Bson, path: &str) -> Result<Value> {
        match raw {
            Bson::Binary(b) => Ok(Value::Binary(BinaryAsset::from_bytes(
                b.bytes.clone(),
                b.subtype,
            ))),
            Bson::String(s) => Ok(Value::Binary(BinaryAsset::from_bytes(
                s.clone().into_bytes(),
                mxrs_bson::BinarySubtype::Generic,
            ))),
            _ => Err(FormsError::InvalidShape {
                shape: "blob",
                path: path.to_string(),
            }),
        }
    }

    fn decode_text(&self, raw: &Bson, path: &str) -> Result<Text> {
        let Bson::Document(doc) = raw else {
            return Err(FormsError::InvalidShape {
                shape: "Text",
                path: path.to_string(),
            });
        };
        if !doc
            .get_str("$Type")
            .map(|t| t.ends_with("$Text"))
            .unwrap_or(false)
        {
            return Err(FormsError::InvalidShape {
                shape: "Text",
                path: path.to_string(),
            });
        }
        let items = match doc.get("Items") {
            Some(Bson::Array(a)) => mxrs_bson::parse_array(Some(a)).items,
            _ => Vec::new(),
        };
        let translations = items
            .iter()
            .map(|item| {
                let Bson::Document(item_doc) = item else {
                    return Err(FormsError::InvalidShape {
                        shape: "Text.Items[]",
                        path: path.to_string(),
                    });
                };
                let language = item_doc.get_str("LanguageCode").ok().map(str::to_string);
                let text = item_doc.get_str("Text").unwrap_or("").to_string();
                Ok(Translation { language, text })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Text::from_translations(translations))
    }

    fn decode_text_template(&self, raw: &Bson, path: &str) -> Result<TextTemplate> {
        let Bson::Document(doc) = raw else {
            return Err(FormsError::InvalidShape {
                shape: "TextTemplate",
                path: path.to_string(),
            });
        };
        let text_raw = doc.get("Text").ok_or_else(|| FormsError::InvalidShape {
            shape: "TextTemplate.Text",
            path: path.to_string(),
        })?;
        let text = self.decode_text(text_raw, &format!("{path}.Text"))?;
        let parameters = match doc.get("Parameters") {
            Some(Bson::Array(a)) => mxrs_bson::parse_array(Some(a))
                .items
                .iter()
                .filter_map(|item| {
                    if let Bson::Document(d) = item {
                        d.get_str("Expression").ok().map(str::to_string)
                    } else {
                        None
                    }
                })
                .collect(),
            _ => Vec::new(),
        };
        Ok(TextTemplate::build(text, parameters))
    }

    /// `decode_attribute_reference`/`decode_entity_reference` themselves
    /// now live in `mxrs_forms_refs` (shared with `mxrs-pluggable` — see
    /// that crate's doc comment for why); this just maps its error type
    /// onto `FormsError` at the boundary.
    fn ref_error(error: mxrs_forms_refs::RefError) -> FormsError {
        let mxrs_forms_refs::RefError::InvalidShape { shape, path } = error;
        FormsError::InvalidShape { shape, path }
    }

    fn decode_condition(&self, raw: &Bson, path: &str) -> Result<Condition> {
        let Bson::Document(doc) = raw else {
            return Err(FormsError::InvalidShape {
                shape: "Condition",
                path: path.to_string(),
            });
        };
        if !doc
            .get_str("$Type")
            .map(|t| t.ends_with("$Condition"))
            .unwrap_or(false)
        {
            return Err(FormsError::InvalidShape {
                shape: "Condition",
                path: path.to_string(),
            });
        }
        let visible = matches!(doc.get("EditableVisible"), Some(Bson::Boolean(true)));
        Ok(Condition::when_value(
            doc.get_str("AttributeValue").unwrap_or(""),
            visible,
        ))
    }

    fn decode_xpath_constraint(&self, raw: &Bson, path: &str) -> Result<XPathConstraint> {
        let Bson::Array(items) = raw else {
            let clause = self.expect_string(raw, path)?;
            return Ok(XPathConstraint::from_clauses(vec![clause]));
        };
        let clauses = mxrs_bson::parse_array(Some(items))
            .items
            .iter()
            .map(|item| match item {
                Bson::String(s) => Ok(s.clone()),
                Bson::Document(doc) if doc.contains_key("XPathConstraint") => Ok(doc.get_str("XPathConstraint").unwrap_or("").to_string()),
                _ => Err(FormsError::Other(format!("structured legacy database constraint at {path} cannot be converted to XPath without entity/attribute context"))),
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(XPathConstraint::from_clauses(clauses))
    }

    fn decode_data_type(&self, raw: &Bson, path: &str) -> Result<DataType> {
        let Bson::Document(doc) = raw else {
            let name = self.expect_string(raw, path)?;
            return Ok(DataType::build(name, None));
        };
        if !doc
            .get_str("$Type")
            .map(|t| t.starts_with("DataTypes$"))
            .unwrap_or(false)
        {
            let name = doc.get_str("$Type").unwrap_or("").to_string();
            return Ok(DataType::build(name, None));
        }
        let type_field = doc.get_str("$Type").unwrap_or("");
        let name = type_field
            .split('$')
            .next_back()
            .unwrap_or("")
            .trim_end_matches("Type")
            .to_string();
        let target_field = match name.as_str() {
            "Object" | "List" => Some("Entity"),
            "Enumeration" => Some("Enumeration"),
            _ => None,
        };
        let mut known: Vec<&str> = vec!["$ID", "$Type"];
        if let Some(f) = target_field {
            known.push(f);
        }
        let mut unknown: Vec<&String> = doc
            .keys()
            .filter(|k| !known.contains(&k.as_str()))
            .collect();
        if !unknown.is_empty() {
            unknown.sort();
            return Err(FormsError::Other(format!(
                "unsupported DataType field(s) at {path}: {}",
                unknown.into_iter().cloned().collect::<Vec<_>>().join(", ")
            )));
        }
        let target = target_field
            .and_then(|f| doc.get_str(f).ok())
            .map(str::to_string);
        Ok(DataType::build(name, target))
    }

    fn expect_string(&self, raw: &Bson, path: &str) -> Result<String> {
        match raw {
            Bson::String(s) => Ok(s.clone()),
            _ => Err(FormsError::InvalidShape {
                shape: "string",
                path: path.to_string(),
            }),
        }
    }

    fn expect_integer(&self, raw: &Bson, path: &str) -> Result<i64> {
        match raw {
            Bson::Int32(i) => Ok(i64::from(*i)),
            Bson::Int64(i) => Ok(*i),
            _ => Err(FormsError::InvalidShape {
                shape: "integer",
                path: path.to_string(),
            }),
        }
    }

    fn expect_boolean(&self, raw: &Bson, path: &str) -> Result<bool> {
        match raw {
            Bson::Boolean(b) => Ok(*b),
            _ => Err(FormsError::InvalidShape {
                shape: "boolean",
                path: path.to_string(),
            }),
        }
    }

    fn internal_type_name(&self, storage_type: &str) -> Result<String> {
        let prefix = INTERNAL_PREFIXES
            .iter()
            .find(|p| storage_type.starts_with(**p))
            .ok_or_else(|| FormsError::NotAFormsStorageType(storage_type.to_string()))?;
        Ok(storage_naming::schema_type_name(
            &storage_type[prefix.len()..],
        ))
    }

    // ── Encode ───────────────────────────────────────────────────────────

    fn encode_node(
        &self,
        node: &Node,
        path: &str,
        local_references: &HashMap<String, String>,
    ) -> Result<Document> {
        let mut document = Document::new();
        document.insert("$ID", self.node_identifier(node, local_references));
        document.insert(
            "$Type",
            format!(
                "Forms${}",
                storage_naming::storage_type_name(&node.schema_type().name)
            ),
        );
        for assignment in node.assignments() {
            let property = &assignment.property;
            let storage = storage_naming::resolve(property).name;
            let field_path = format!("{path}.{storage}");
            let encoded =
                self.encode_property(property, &assignment.value, &field_path, local_references)?;
            document.insert(storage.clone(), encoded);
            if property.type_name == "Expression"
                && matches!(
                    property.declared_by.as_str(),
                    "ConditionalSettings" | "WidgetValidation"
                )
            {
                document.insert("ExpressionModel", no_expression_document());
            }
        }
        Ok(document)
    }

    fn encode_property(
        &self,
        property: &Property,
        value: &Value,
        path: &str,
        local_references: &HashMap<String, String>,
    ) -> Result<Bson> {
        if property.many() {
            let Value::List(items) = value else {
                return Err(FormsError::ExpectedArray {
                    type_name: property.declared_by.clone(),
                    property: property.name.clone(),
                });
            };
            let encoded = items
                .iter()
                .enumerate()
                .map(|(i, item)| self.encode_one(item, &format!("{path}[{i}]"), local_references))
                .collect::<Result<Vec<_>>>()?;
            return Ok(Bson::Array(mxrs_bson::build_array(
                encoded,
                collection_marker(property),
            )));
        }
        self.encode_one(value, path, local_references)
    }

    fn encode_one(
        &self,
        value: &Value,
        path: &str,
        local_references: &HashMap<String, String>,
    ) -> Result<Bson> {
        match value {
            Value::Null => Ok(Bson::Null),
            Value::Reference(r) => self.encode_reference(r, path, local_references),
            Value::Enum(e) => Ok(Bson::String(e.value.clone())),
            Value::Node(n) => Ok(Bson::Document(self.encode_node(
                n,
                path,
                local_references,
            )?)),
            Value::Text(t) => Ok(Bson::Document(encode_text(t))),
            Value::TextTemplate(t) => Ok(Bson::Document(encode_text_template(t, path))),
            Value::AttributeReference(a) => Ok(Bson::Document(encode_attribute_reference(a))),
            Value::EntityReference(e) => Ok(Bson::Document(encode_entity_reference(e))),
            Value::DataType(d) => Ok(Bson::Document(encode_data_type(d))),
            Value::Condition(c) => Ok(Bson::Document(encode_condition(c))),
            Value::Binary(b) => Ok(Bson::Binary(mxrs_bson::Binary {
                subtype: b.subtype,
                bytes: b.bytes().map_err(|e| FormsError::Other(e.to_string()))?,
            })),
            Value::Size(s) => {
                let mut d = Document::new();
                d.insert("Width", s.width);
                d.insert("Height", s.height);
                Ok(Bson::Document(d))
            }
            Value::Expression(e) => Ok(Bson::String(e.source.clone())),
            Value::XPathConstraint(x) => Ok(Bson::String(x.source())),
            Value::String(s) => Ok(Bson::String(s.clone())),
            Value::Integer(i) => Ok(Bson::Int64(*i)),
            Value::Boolean(b) => Ok(Bson::Boolean(*b)),
            Value::List(_) => Err(FormsError::Other(format!(
                "unexpected nested list at {path}"
            ))),
            Value::Pluggable(_) => Err(FormsError::PluggableEncodeNotSupported {
                path: path.to_string(),
            }),
        }
    }

    fn encode_reference(
        &self,
        reference: &Reference,
        path: &str,
        local_references: &HashMap<String, String>,
    ) -> Result<Bson> {
        if reference.kind != ReferenceKind::ById {
            return Ok(Bson::String(reference.target.clone()));
        }
        if let Some(local) = local_references.get(&reference.target) {
            return Ok(Bson::String(local.clone()));
        }
        match &self.reference_encoder {
            Some(f) => f(&reference.target, path),
            None => Err(FormsError::UnresolvedStorageReference {
                path: path.to_string(),
            }),
        }
    }

    fn node_identifier(&self, node: &Node, local_references: &HashMap<String, String>) -> String {
        if let Ok(Some(Value::String(name))) = node.fetch("name")
            && let Some(id) = local_references.get(name)
        {
            return id.clone();
        }
        uuid::Uuid::new_v4().to_string()
    }

    /// Pre-assigns an id to every uniquely-named node in the tree, so a
    /// by-id reference to "the node named X" resolves consistently
    /// regardless of which subtree [`Self::encode_node`] happens to visit
    /// first. Ambiguously-named (or unnamed) nodes are left out — nothing
    /// can address them by name anyway, so they just get a fresh random id
    /// when [`Self::node_identifier`] encounters them directly.
    fn collect_local_reference_names(&self, root: &Node) -> HashMap<String, String> {
        let mut named: HashMap<String, Vec<String>> = HashMap::new();
        walk_nodes(root, &mut |node| {
            if node.schema_type().property("name", true).is_some()
                && let Ok(Some(Value::String(name))) = node.fetch("name")
                && !name.is_empty()
            {
                named
                    .entry(name.clone())
                    .or_default()
                    .push(uuid::Uuid::new_v4().to_string());
            }
        });
        named
            .into_iter()
            .filter_map(|(name, mut ids)| {
                if ids.len() == 1 {
                    Some((name, ids.pop().unwrap()))
                } else {
                    None
                }
            })
            .collect()
    }
}

/// Implements [`mxrs_pluggable::EmbeddedFormsDecoder`] for [`MprCodec`],
/// closing over the `local_references` map live for whatever top-level
/// `decode` call is in progress — see `decode_pluggable`'s call site and
/// mxrb's own "nested pluggable values share the root's semantic
/// reference maps" comment on `decode_embedded`. A plain `&MprCodec`
/// can't implement the trait directly since the trait has no way to pass
/// that extra map through `decode_embedded`'s fixed two-argument shape.
struct EmbeddedDecoder<'a> {
    codec: &'a MprCodec,
    local_references: &'a HashMap<String, String>,
}

impl mxrs_pluggable::EmbeddedFormsDecoder for EmbeddedDecoder<'_> {
    type Node = Node;
    type Error = FormsError;

    fn decode_embedded(&self, document: &Document, path: &str) -> Result<Node> {
        self.codec
            .decode_node(document, path, self.local_references)
    }
}

/// A standalone (non-nested) [`mxrs_pluggable::EmbeddedFormsDecoder`] impl
/// on `MprCodec` itself — the public entry point for anyone decoding a
/// pluggable widget's instance data directly (e.g.
/// `mxrs-pluggable`'s own `instance_oracle.rs`), as opposed to
/// `decode_pluggable`'s internal nested case (which needs the
/// outer document's `local_references`, via the private `EmbeddedDecoder`
/// wrapper above). Computes `local_references` fresh from whatever
/// document is passed in, same as top-level `MprCodec::decode`.
impl mxrs_pluggable::EmbeddedFormsDecoder for MprCodec {
    type Node = Node;
    type Error = FormsError;

    fn decode_embedded(&self, document: &Document, path: &str) -> Result<Node> {
        let local_references = storage_reference_names(document);
        self.decode_node(document, path, &local_references)
    }
}

fn walk_nodes(node: &Node, visit: &mut impl FnMut(&Node)) {
    visit(node);
    for assignment in node.assignments() {
        walk_value(&assignment.value, visit);
    }
}

fn walk_value(value: &Value, visit: &mut impl FnMut(&Node)) {
    match value {
        Value::Node(n) => walk_nodes(n, visit),
        Value::List(items) => {
            for item in items {
                walk_value(item, visit);
            }
        }
        _ => {}
    }
}

fn collection_marker(property: &Property) -> i32 {
    if property.reference == Some(ReferenceKind::ByName) {
        return 1;
    }
    COLLECTION_MARKERS
        .iter()
        .find(|((d, n), _)| *d == property.declared_by && *n == property.name)
        .map(|(_, m)| *m)
        .unwrap_or(2)
}

fn encode_text(text: &Text) -> Document {
    let items: Vec<Bson> = text
        .translations
        .iter()
        .map(|t| {
            let mut d = Document::new();
            d.insert("$ID", uuid::Uuid::new_v4().to_string());
            d.insert("$Type", "Texts$Translation");
            d.insert("LanguageCode", t.language.clone().unwrap_or_default());
            d.insert("Text", t.text.clone());
            Bson::Document(d)
        })
        .collect();
    let mut document = Document::new();
    document.insert("$ID", uuid::Uuid::new_v4().to_string());
    document.insert("$Type", "Texts$Text");
    document.insert("Items", mxrs_bson::build_array(items, 3));
    document
}

fn encode_text_template(template: &TextTemplate, path: &str) -> Document {
    let parameters: Vec<Bson> = template
        .parameters
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let mut d = Document::new();
            d.insert("$ID", uuid::Uuid::new_v4().to_string());
            d.insert("$Type", "Microflows$TemplateParameter");
            d.insert("Expression", p.expression.source.clone());
            let _ = i;
            d.insert("ExpressionModel", no_expression_document());
            Bson::Document(d)
        })
        .collect();
    let mut document = Document::new();
    document.insert("$ID", uuid::Uuid::new_v4().to_string());
    document.insert("$Type", "Microflows$TextTemplate");
    document.insert("Text", Bson::Document(encode_text(&template.text)));
    document.insert("Parameters", mxrs_bson::build_array(parameters, 2));
    let _ = path;
    document
}

fn encode_data_type(data_type: &DataType) -> Document {
    let mut document = Document::new();
    document.insert("$ID", uuid::Uuid::new_v4().to_string());
    document.insert(
        "$Type",
        format!("DataTypes${}Type", data_type.name.trim_end_matches("Type")),
    );
    let target_field = match data_type.name.as_str() {
        "Object" | "List" => Some("Entity"),
        "Enumeration" => Some("Enumeration"),
        _ => None,
    };
    if let (Some(field), Some(target)) = (target_field, &data_type.target) {
        document.insert(field, target.clone());
    }
    document
}

fn encode_condition(condition: &Condition) -> Document {
    let mut document = Document::new();
    document.insert("$ID", uuid::Uuid::new_v4().to_string());
    document.insert("$Type", "Enumerations$Condition");
    document.insert("AttributeValue", condition.attribute_value.clone());
    document.insert("EditableVisible", condition.editable_visible);
    document
}

fn no_expression_document() -> Bson {
    let mut d = Document::new();
    d.insert("$ID", uuid::Uuid::new_v4().to_string());
    d.insert("$Type", "Expressions$NoExpression");
    Bson::Document(d)
}

fn storage_reference_names(document: &Document) -> HashMap<String, String> {
    let mut entries = HashMap::new();
    walk_storage_doc(document, "$", &mut entries);
    entries
}

fn walk_storage_doc(doc: &Document, path: &str, entries: &mut HashMap<String, String>) {
    if let Some(identifier) = doc.get("$ID").and_then(mxrs_bson::extract_id) {
        let name = doc.get_str("Name").unwrap_or("").to_string();
        entries.insert(
            identifier,
            if name.is_empty() {
                path.to_string()
            } else {
                name
            },
        );
    }
    for (key, child) in doc {
        if key == "$ID" || key == "TypePointer" {
            continue;
        }
        walk_storage_value(child, &format!("{path}.{key}"), entries);
    }
}

fn walk_storage_value(value: &Bson, path: &str, entries: &mut HashMap<String, String>) {
    match value {
        Bson::Document(doc) => walk_storage_doc(doc, path, entries),
        Bson::Array(items) => {
            for (i, child) in mxrs_bson::parse_array(Some(items)).items.iter().enumerate() {
                walk_storage_value(child, &format!("{path}[{i}]"), entries);
            }
        }
        _ => {}
    }
}
