use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use mxrs_bson::{Bson, Document};

use crate::source_model::{SourceModel, SourceUnit};
use crate::Result;

const TYPE_MAP: &[(&str, &str)] = &[
    ("DomainModels$StringAttributeType", "java.lang.String"),
    ("DomainModels$HashStringAttributeType", "java.lang.String"),
    ("DomainModels$HashedStringAttributeType", "java.lang.String"),
    ("DomainModels$IntegerAttributeType", "java.lang.Integer"),
    ("DomainModels$LongAttributeType", "java.lang.Long"),
    ("DomainModels$AutoNumberAttributeType", "java.lang.Long"),
    ("DomainModels$DecimalAttributeType", "java.math.BigDecimal"),
    ("DomainModels$BooleanAttributeType", "java.lang.Boolean"),
    ("DomainModels$DateTimeAttributeType", "java.util.Date"),
    ("DomainModels$BinaryAttributeType", "byte[]"),
];

const CONSTANT_TYPE_MAP: &[(&str, &str)] = &[
    ("DataTypes$StringType", "java.lang.String"),
    ("DataTypes$IntegerType", "java.lang.Long"),
    ("DataTypes$DecimalType", "java.math.BigDecimal"),
    ("DataTypes$BooleanType", "java.lang.Boolean"),
    ("DataTypes$DateTimeType", "java.util.Date"),
];

#[derive(Debug, Clone)]
struct DomainUnit {
    module_name: String,
    document: Document,
}

#[derive(Debug, Clone)]
struct EntityRecord {
    unit: DomainUnit,
    entity: Document,
}

/// Generates only Java artifacts referenced by existing project sources.
pub struct JavaProxyGenerator {
    source: SourceModel,
    project_root: PathBuf,
    entities: HashMap<String, EntityRecord>,
    system_documents: Vec<Document>,
}

impl JavaProxyGenerator {
    pub fn new(mpr_path: impl AsRef<Path>, project_root: impl AsRef<Path>) -> Result<Self> {
        let mpr_path = std::path::absolute(mpr_path.as_ref())?;
        let source = SourceModel::read(&mpr_path)?;
        let system_documents = mxrs_schema::system_model_documents(&source.version)?;
        let mut domains: Vec<DomainUnit> = source
            .units_of("DomainModels$DomainModel")
            .filter_map(|unit| {
                Some(DomainUnit {
                    module_name: unit.module_name.clone()?,
                    document: unit.document.clone(),
                })
            })
            .collect();
        if let Some(document) = system_documents
            .iter()
            .find(|document| native_type(document) == Some("DomainModels$DomainModel"))
        {
            domains.push(DomainUnit {
                module_name: "System".into(),
                document: document.clone(),
            });
        }
        let entities = entity_index(&domains);
        Ok(Self {
            source,
            project_root: std::path::absolute(project_root.as_ref())?,
            entities,
            system_documents,
        })
    }

    /// Writes missing generated files and returns their count. User-owned or
    /// previously generated files are deliberately left untouched.
    pub fn generate(&self) -> Result<usize> {
        let mut generated = usize::from(self.write_user_actions_registrar()?);
        let mut source_text = self.java_source_text()?;

        for module in self.microflow_modules() {
            if referenced(
                &source_text,
                &format!("{}.proxies.microflows.Microflows", java_package(&module)),
            ) && self.write_microflows(&module, &source_text)?
            {
                generated += 1;
            }
        }
        if generated > 0 {
            source_text = self.java_source_text()?;
        }

        let requested = self.requested_entities(&source_text);
        let mut entity_count = 0;
        for qualified in requested {
            if self.write_entity(&qualified)? {
                entity_count += 1;
            }
        }
        generated += entity_count;
        if entity_count > 0 {
            source_text = self.java_source_text()?;
        }

        for (module, document) in self.enumeration_units() {
            let qualified = format!(
                "{}.proxies.{}",
                java_package(&module),
                document_name(&document)
            );
            if referenced(&source_text, &qualified) && self.write_enum(&module, &document)? {
                generated += 1;
            }
        }
        for module in self.constant_modules() {
            let qualified = format!("{}.proxies.constants.Constants", java_package(&module));
            if referenced(&source_text, &qualified) && self.write_constants(&module)? {
                generated += 1;
            }
        }
        Ok(generated)
    }

    /// Qualified entity names available to proxy generation, including the
    /// embedded System module. Useful to explain why a reference was or was
    /// not materialized without exposing raw model documents.
    pub fn indexed_entity_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.entities.keys().cloned().collect();
        names.sort();
        names
    }

    fn java_source_text(&self) -> Result<String> {
        let mut paths = Vec::new();
        collect_java_files(&self.project_root.join("javasource"), &mut paths)?;
        paths.sort();
        let mut text = String::new();
        for path in paths {
            let bytes = fs::read(path)?;
            text.push_str(&String::from_utf8_lossy(&bytes));
            text.push('\n');
        }
        Ok(text)
    }

    fn write_user_actions_registrar(&self) -> Result<bool> {
        let classes = self.user_action_classes();
        if classes.is_empty() {
            return Ok(false);
        }
        let registrations = classes
            .iter()
            .map(|class_name| format!("    registrator.registerUserAction({class_name}.class);"))
            .collect::<Vec<_>>()
            .join("\n");
        let source = format!(
            "// Generated natively by mxrs from Java action documents and sources.\n\
             package system;\n\n\
             public class UserActionsRegistrar {{\n\
               public void registerActions(com.mendix.core.actionmanagement.IActionRegistrator registrator) {{\n\
             {registrations}\n\
               }}\n\
             }}\n"
        );
        write_missing(
            &self
                .project_root
                .join("javasource/system/UserActionsRegistrar.java"),
            &source,
        )
    }

    fn user_action_classes(&self) -> Vec<String> {
        let mut classes: Vec<_> = self
            .source
            .units_of("JavaActions$JavaAction")
            .filter_map(|unit| {
                let name = document_name(&unit.document);
                let module = unit.module_name.as_deref()?;
                if name.is_empty() {
                    return None;
                }
                let package = java_package(module);
                self.project_root
                    .join("javasource")
                    .join(&package)
                    .join("actions")
                    .join(format!("{name}.java"))
                    .is_file()
                    .then(|| format!("{package}.actions.{name}"))
            })
            .collect();
        classes.sort();
        classes.dedup();
        classes
    }

    fn requested_entities(&self, text: &str) -> Vec<String> {
        let mut result: HashSet<String> = self
            .entities
            .keys()
            .filter(|qualified| referenced(text, &java_proxy_name(qualified)))
            .cloned()
            .collect();
        loop {
            let before = result.len();
            for name in result.clone() {
                self.add_entity_dependencies(&name, &mut result);
            }
            if result.len() == before {
                break;
            }
        }
        let mut values: Vec<_> = result.into_iter().collect();
        values.sort();
        values
    }

    fn add_entity_dependencies(&self, name: &str, result: &mut HashSet<String>) {
        let Some(record) = self.entities.get(name) else {
            return;
        };
        let parent = generalization(&record.entity);
        if self.entities.contains_key(&parent) {
            result.insert(parent);
        }
        for association in associations_for(&record.unit, &record.entity) {
            let child = self.association_child_qualified(&record.unit, association);
            if self.entities.contains_key(&child) {
                result.insert(child);
            }
        }
    }

    fn write_entity(&self, qualified: &str) -> Result<bool> {
        let Some(record) = self.entities.get(qualified) else {
            return Ok(false);
        };
        let path = self.proxy_path(
            &record.unit.module_name,
            &[format!("{}.java", entity_name(&record.entity))],
        );
        write_missing(&path, &self.entity_source(record))
    }

    fn entity_source(&self, record: &EntityRecord) -> String {
        let unit = &record.unit;
        let entity = &record.entity;
        let name = entity_name(entity);
        let qualified = entity_qualified_name(unit, entity);
        let java_name = java_proxy_name(&qualified);
        let parent = generalization(entity);
        let local_parent = self
            .entities
            .contains_key(&parent)
            .then(|| java_proxy_name(&parent));
        let mut members: Vec<(String, String)> = attributes(entity)
            .into_iter()
            .map(|attribute| {
                let name = string_any(attribute, &["Name", "name"]);
                (name.clone(), name)
            })
            .collect();
        members.extend(
            associations_for(unit, entity)
                .into_iter()
                .map(|association| {
                    (
                        association_name(association),
                        association_qualified_name(unit, association),
                    )
                }),
        );
        let inheritance = local_parent
            .as_ref()
            .map(|parent| format!("extends {parent}"))
            .unwrap_or_else(|| {
                "implements com.mendix.systemwideinterfaces.core.IEntityProxy".into()
            });
        let storage = if local_parent.is_some() {
            String::new()
        } else {
            "  private final com.mendix.systemwideinterfaces.core.IMendixObject mendixObject;\n  private final com.mendix.systemwideinterfaces.core.IContext context;\n".into()
        };
        let constructor_body: String = if local_parent.is_some() {
            "super(context, mendixObject);".into()
        } else {
            "if (mendixObject == null)\n      throw new java.lang.IllegalArgumentException(\"The given object cannot be null.\");\n    this.mendixObject = mendixObject;\n    this.context = context;".into()
        };
        let mut accessors = attributes(entity)
            .into_iter()
            .map(|attribute| self.attribute_methods(unit, attribute))
            .collect::<Vec<_>>()
            .join("\n");
        accessors.push_str(
            &associations_for(unit, entity)
                .into_iter()
                .map(|association| self.association_methods(unit, association))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let base_methods = if local_parent.is_some() {
            String::new()
        } else {
            "  @java.lang.Override public com.mendix.systemwideinterfaces.core.IMendixObject getMendixObject() { return mendixObject; }\n  public com.mendix.systemwideinterfaces.core.IContext getContext() { return context; }\n  @java.lang.Override public boolean equals(Object other) {\n    return other == this || (other != null && getClass().equals(other.getClass()) &&\n      mendixObject.equals(((com.mendix.systemwideinterfaces.core.IEntityProxy) other).getMendixObject()));\n  }\n  @java.lang.Override public int hashCode() { return mendixObject.hashCode(); }\n".into()
        };
        let member_names = members
            .into_iter()
            .map(|(member, meta)| format!("{member}(\"{meta}\")"))
            .collect::<Vec<_>>()
            .join(",\n    ");
        format!(
            "// Generated natively by mxrs from the MPR model.\n\
             package {}.proxies;\n\n\
             public class {name} {inheritance} {{\n\
             {storage}\
               public static final java.lang.String entityName = \"{qualified}\";\n\
               public enum MemberNames {{\n    {member_names};\n\
                 private final java.lang.String metaName;\n\
                 MemberNames(java.lang.String value) {{ metaName = value; }}\n\
                 @java.lang.Override public java.lang.String toString() {{ return metaName; }}\n\
               }}\n\n\
               public {name}(com.mendix.systemwideinterfaces.core.IContext context) {{\n\
                 this(context, com.mendix.core.Core.instantiate(context, entityName));\n\
               }}\n\
               protected {name}(com.mendix.systemwideinterfaces.core.IContext context,\n\
                   com.mendix.systemwideinterfaces.core.IMendixObject mendixObject) {{\n\
                 {constructor_body}\n\
               }}\n\
               public static {java_name} initialize(com.mendix.systemwideinterfaces.core.IContext context,\n\
                   com.mendix.systemwideinterfaces.core.IMendixObject mendixObject) {{\n\
                 return new {java_name}(context, mendixObject);\n\
               }}\n\
               public static {java_name} load(com.mendix.systemwideinterfaces.core.IContext context,\n\
                   com.mendix.systemwideinterfaces.core.IMendixIdentifier id) throws com.mendix.core.CoreException {{\n\
                 return initialize(context, com.mendix.core.Core.retrieveId(context, id));\n\
               }}\n\
             {accessors}\n\
             {base_methods}\
               public static java.lang.String getType() {{ return entityName; }}\n\
             }}\n",
            java_package(&unit.module_name)
        )
    }

    fn attribute_methods(&self, unit: &DomainUnit, attribute: &Document) -> String {
        let name = string_any(attribute, &["Name", "name"]);
        let attribute_type = attribute
            .get_document("NewType")
            .or_else(|_| attribute.get_document("Type"))
            .or_else(|_| attribute.get_document("newType"))
            .or_else(|_| attribute.get_document("type"))
            .cloned()
            .unwrap_or_default();
        let java_type = attribute_java_type(unit, &attribute_type);
        let is_enum = native_type(&attribute_type) == Some("DomainModels$EnumerationAttributeType");
        let read = if is_enum {
            format!("Object value = getMendixObject().getValue(context, MemberNames.{name}.toString());\n    return value == null ? null : {java_type}.valueOf((java.lang.String) value);")
        } else {
            format!("return ({java_type}) getMendixObject().getValue(context, MemberNames.{name}.toString());")
        };
        let value = if is_enum {
            "value == null ? null : value.toString()"
        } else {
            "value"
        };
        format!(
            "  public final {java_type} get{name}() {{ return get{name}(getContext()); }}\n\
               public final {java_type} get{name}(com.mendix.systemwideinterfaces.core.IContext context) {{\n    {read}\n  }}\n\
               public final void set{name}({java_type} value) {{ set{name}(getContext(), value); }}\n\
               public final void set{name}(com.mendix.systemwideinterfaces.core.IContext context, {java_type} value) {{\n\
                 getMendixObject().setValue(context, MemberNames.{name}.toString(), {value});\n\
               }}\n"
        )
    }

    fn association_methods(&self, unit: &DomainUnit, association: &Document) -> String {
        let name = association_name(association);
        let child_type = self.association_child_type(unit, association);
        match string_any(association, &["Type", "type"]).as_str() {
            "Reference" => format!(
                "  public final {child_type} get{name}() throws com.mendix.core.CoreException {{ return get{name}(getContext()); }}\n\
                   public final {child_type} get{name}(com.mendix.systemwideinterfaces.core.IContext context) throws com.mendix.core.CoreException {{\n\
                     com.mendix.systemwideinterfaces.core.IMendixIdentifier id = getMendixObject().getValue(context, MemberNames.{name}.toString());\n\
                     return id == null ? null : {child_type}.load(context, id);\n\
                   }}\n\
                   public final void set{name}({child_type} value) {{ set{name}(getContext(), value); }}\n\
                   public final void set{name}(com.mendix.systemwideinterfaces.core.IContext context, {child_type} value) {{\n\
                     getMendixObject().setValue(context, MemberNames.{name}.toString(), value == null ? null : value.getMendixObject().getId());\n\
                   }}\n"
            ),
            "ReferenceSet" => format!(
                "  public final java.util.List<{child_type}> get{name}() throws com.mendix.core.CoreException {{ return get{name}(getContext()); }}\n\
                   public final java.util.List<{child_type}> get{name}(com.mendix.systemwideinterfaces.core.IContext context) throws com.mendix.core.CoreException {{\n\
                     java.util.List<com.mendix.systemwideinterfaces.core.IMendixIdentifier> ids = getMendixObject().getValue(context, MemberNames.{name}.toString());\n\
                     if (ids == null) return java.util.Collections.emptyList();\n\
                     java.util.List<{child_type}> result = new java.util.ArrayList<>();\n\
                     for (com.mendix.systemwideinterfaces.core.IMendixIdentifier id : ids) result.add({child_type}.load(context, id));\n\
                     return result;\n\
                   }}\n\
                   public final void set{name}(java.util.List<{child_type}> values) {{ set{name}(getContext(), values); }}\n\
                   public final void set{name}(com.mendix.systemwideinterfaces.core.IContext context, java.util.List<{child_type}> values) {{\n\
                     java.util.List<com.mendix.systemwideinterfaces.core.IMendixIdentifier> ids = values == null ? null : values.stream()\n\
                       .map(value -> value.getMendixObject().getId()).collect(java.util.stream.Collectors.toList());\n\
                     getMendixObject().setValue(context, MemberNames.{name}.toString(), ids);\n\
                   }}\n"
            ),
            _ => String::new(),
        }
    }

    fn association_child_type(&self, unit: &DomainUnit, association: &Document) -> String {
        let qualified = self.association_child_qualified(unit, association);
        if !qualified.is_empty() && self.entities.contains_key(&qualified) {
            java_proxy_name(&qualified)
        } else {
            "com.mendix.systemwideinterfaces.core.IEntityProxy".into()
        }
    }

    fn association_child_qualified(&self, unit: &DomainUnit, association: &Document) -> String {
        let qualified = string_any(association, &["Child", "child"]);
        if !qualified.is_empty() {
            return qualified;
        }
        let wanted = value_any(association, &["ChildPointer", "ChildID", "childId"])
            .and_then(mxrs_bson::extract_id);
        let Some(wanted) = wanted else {
            return String::new();
        };
        self.entities
            .values()
            .find(|record| {
                record
                    .entity
                    .get("$ID")
                    .and_then(mxrs_bson::extract_id)
                    .as_deref()
                    == Some(wanted.as_str())
            })
            .map(|record| entity_qualified_name(&record.unit, &record.entity))
            .unwrap_or_else(|| {
                let _ = unit;
                String::new()
            })
    }

    fn enumeration_units(&self) -> Vec<(String, Document)> {
        let mut units: Vec<_> = self
            .source
            .units_of("Enumerations$Enumeration")
            .filter_map(|unit| Some((unit.module_name.clone()?, unit.document.clone())))
            .collect();
        units.extend(
            self.system_documents
                .iter()
                .filter(|document| native_type(document) == Some("Enumerations$Enumeration"))
                .cloned()
                .map(|document| ("System".into(), document)),
        );
        units
    }

    fn write_enum(&self, module: &str, document: &Document) -> Result<bool> {
        let names = docs_any(document, &["Values", "values"])
            .into_iter()
            .map(|value| string(value, "Name"))
            .collect::<Vec<_>>();
        let name = document_name(document);
        let source = format!(
            "// Generated natively by mxrs from the MPR model.\npackage {}.proxies;\npublic enum {name} {{ {} }}\n",
            java_package(module),
            names.join(", ")
        );
        write_missing(&self.proxy_path(module, &[format!("{name}.java")]), &source)
    }

    fn constant_modules(&self) -> Vec<String> {
        unique_modules(self.source.units_of("Constants$Constant"))
    }

    fn write_constants(&self, module: &str) -> Result<bool> {
        let methods = self
            .source
            .units_of("Constants$Constant")
            .filter(|unit| unit.module_name.as_deref() == Some(module))
            .map(|unit| {
                let type_name = unit
                    .document
                    .get_document("Type")
                    .ok()
                    .and_then(native_type)
                    .unwrap_or_default();
                let java_type = map_type(CONSTANT_TYPE_MAP, type_name, "java.lang.Object");
                let name = string(&unit.document, "Name");
                format!("public static {java_type} get{name}() {{ return ({java_type}) com.mendix.core.Core.getConfiguration().getConstantValue(\"{module}.{name}\"); }}")
            })
            .collect::<Vec<_>>()
            .join("\n  ");
        let source = format!(
            "// Generated natively by mxrs from the MPR model.\npackage {}.proxies.constants;\npublic final class Constants {{\n  private Constants() {{}}\n  {methods}\n}}\n",
            java_package(module)
        );
        write_missing(
            &self.proxy_path(module, &["constants".into(), "Constants.java".into()]),
            &source,
        )
    }

    fn microflow_modules(&self) -> Vec<String> {
        unique_modules(self.source.units_of("Microflows$Microflow"))
    }

    fn write_microflows(&self, module: &str, text: &str) -> Result<bool> {
        let methods = self
            .source
            .units_of("Microflows$Microflow")
            .filter(|unit| unit.module_name.as_deref() == Some(module))
            .filter(|unit| {
                let needle = format!(
                    "Microflows.{}",
                    lower_camel(&string(&unit.document, "Name"))
                );
                referenced(text, &needle)
            })
            .map(|unit| microflow_method(module, &unit.document))
            .collect::<Vec<_>>();
        if methods.is_empty() {
            return Ok(false);
        }
        let source = format!(
            "// Generated natively by mxrs from the MPR model.\npackage {}.proxies.microflows;\npublic final class Microflows {{\n  private Microflows() {{}}\n{}\n}}\n",
            java_package(module),
            methods.join("\n")
        );
        write_missing(
            &self.proxy_path(module, &["microflows".into(), "Microflows.java".into()]),
            &source,
        )
    }

    fn proxy_path(&self, module: &str, parts: &[String]) -> PathBuf {
        let mut path = self
            .project_root
            .join("javasource")
            .join(java_package(module))
            .join("proxies");
        for part in parts {
            path.push(part);
        }
        path
    }
}

fn entity_index(domains: &[DomainUnit]) -> HashMap<String, EntityRecord> {
    let mut entities = HashMap::new();
    for unit in domains {
        for entity in docs_any(&unit.document, &["Entities", "entities"]) {
            entities.insert(
                entity_qualified_name(unit, entity),
                EntityRecord {
                    unit: unit.clone(),
                    entity: entity.clone(),
                },
            );
        }
    }
    entities
}

fn microflow_method(module: &str, document: &Document) -> String {
    let parameters = microflow_parameters(document);
    let declarations = parameters
        .iter()
        .map(|parameter| {
            let name = string(parameter, "Name");
            let variable_type = parameter.get_document("VariableType").ok();
            format!(
                "{} _{}",
                java_data_type(variable_type, false),
                lower_camel(&name)
            )
        })
        .collect::<Vec<_>>();
    let arguments = parameters
        .iter()
        .map(|parameter| {
            let name = string(parameter, "Name");
            format!(".withParam(\"{name}\", _{})", lower_camel(&name))
        })
        .collect::<String>();
    let return_type_doc = document.get_document("MicroflowReturnType").ok();
    let return_type = java_data_type(return_type_doc, true);
    let result = microflow_result(return_type_doc);
    let suffix = if declarations.is_empty() {
        String::new()
    } else {
        format!(",\n        {}", declarations.join(",\n        "))
    };
    let name = string(document, "Name");
    format!(
        "  public static {return_type} {}(\n      com.mendix.systemwideinterfaces.core.IContext context{suffix}) {{\n    Object result = com.mendix.core.Core.microflowCall(\"{module}.{name}\"){arguments}.execute(context);\n    {result}\n  }}",
        lower_camel(&name)
    )
}

fn microflow_parameters(document: &Document) -> Vec<Document> {
    document
        .get_document("ObjectCollection")
        .ok()
        .map(|collection| docs_any(collection, &["Objects", "objects"]))
        .unwrap_or_default()
        .into_iter()
        .filter(|object| native_type(object) == Some("Microflows$MicroflowParameter"))
        .cloned()
        .collect()
}

fn java_data_type(data_type: Option<&Document>, return_type: bool) -> String {
    match data_type.and_then(native_type) {
        Some("DataTypes$BooleanType") if return_type => "boolean".into(),
        Some("DataTypes$BooleanType") => "java.lang.Boolean".into(),
        Some("DataTypes$IntegerType" | "DataTypes$LongType") => "java.lang.Long".into(),
        Some("DataTypes$DecimalType") => "java.math.BigDecimal".into(),
        Some("DataTypes$DateTimeType") => "java.util.Date".into(),
        Some("DataTypes$StringType") => "java.lang.String".into(),
        Some("DataTypes$ObjectType") => java_proxy_name(
            &data_type
                .map(|value| string(value, "Entity"))
                .unwrap_or_default(),
        ),
        Some("DataTypes$VoidType") | None => "void".into(),
        _ => "java.lang.Object".into(),
    }
}

fn microflow_result(data_type: Option<&Document>) -> String {
    match data_type.and_then(native_type) {
        Some("DataTypes$BooleanType") => "return (boolean) result;".into(),
        Some("DataTypes$ObjectType") => {
            let proxy = java_proxy_name(
                &data_type
                    .map(|value| string(value, "Entity"))
                    .unwrap_or_default(),
            );
            format!("return result == null ? null : {proxy}.initialize(context, (com.mendix.systemwideinterfaces.core.IMendixObject) result);")
        }
        Some("DataTypes$VoidType") | None => "return;".into(),
        _ => format!("return ({}) result;", java_data_type(data_type, true)),
    }
}

fn attribute_java_type(unit: &DomainUnit, data_type: &Document) -> String {
    let type_name = native_type(data_type).unwrap_or_default();
    if type_name != "DomainModels$EnumerationAttributeType" {
        return map_type(TYPE_MAP, type_name, "java.lang.Object").into();
    }
    let mut reference = string_any(data_type, &["Enumeration", "enumeration"]);
    if !reference.contains('.') {
        reference = format!("{}.{}", unit.module_name, reference);
    }
    java_proxy_name(&reference)
}

fn associations_for<'a>(unit: &'a DomainUnit, entity: &Document) -> Vec<&'a Document> {
    let id = entity.get("$ID").and_then(mxrs_bson::extract_id);
    docs_any(&unit.document, &["Associations", "associations"])
        .into_iter()
        .chain(docs_any(
            &unit.document,
            &["CrossAssociations", "crossAssociations"],
        ))
        .filter(|association| {
            value_any(association, &["ParentPointer", "ParentID", "parentId"])
                .and_then(mxrs_bson::extract_id)
                == id
        })
        .collect()
}

fn attributes(entity: &Document) -> Vec<&Document> {
    docs_any(entity, &["Attributes", "attributes"])
}

fn docs_any<'a>(document: &'a Document, keys: &[&str]) -> Vec<&'a Document> {
    let Some(Bson::Array(values)) = keys.iter().find_map(|key| document.get(*key)) else {
        return Vec::new();
    };
    let start = usize::from(matches!(
        values.first(),
        Some(Bson::Int32(_) | Bson::Int64(_))
    ));
    values[start..]
        .iter()
        .filter_map(|value| match value {
            Bson::Document(document) => Some(document),
            _ => None,
        })
        .collect()
}

fn native_type(document: &Document) -> Option<&str> {
    document.get_str("$Type").ok()
}

fn string(document: &Document, key: &str) -> String {
    document.get_str(key).unwrap_or_default().to_string()
}

fn string_any(document: &Document, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| document.get_str(*key).ok())
        .unwrap_or_default()
        .to_string()
}

fn value_any<'a>(document: &'a Document, keys: &[&str]) -> Option<&'a Bson> {
    keys.iter().find_map(|key| document.get(*key))
}

fn entity_name(entity: &Document) -> String {
    string_any(
        entity,
        &["Name", "name", "UnqualifiedName", "unqualifiedName"],
    )
}

fn entity_qualified_name(unit: &DomainUnit, entity: &Document) -> String {
    let qualified = string_any(entity, &["QualifiedName", "$QualifiedName"]);
    if qualified.is_empty() {
        format!("{}.{}", unit.module_name, entity_name(entity))
    } else {
        qualified
    }
}

fn association_name(association: &Document) -> String {
    let name = string_any(
        association,
        &["Name", "name", "UnqualifiedName", "unqualifiedName"],
    );
    if !name.is_empty() {
        return name;
    }
    string(association, "QualifiedName")
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn association_qualified_name(unit: &DomainUnit, association: &Document) -> String {
    let qualified = string_any(association, &["QualifiedName", "$QualifiedName"]);
    if qualified.is_empty() {
        format!("{}.{}", unit.module_name, association_name(association))
    } else {
        qualified
    }
}

fn generalization(entity: &Document) -> String {
    entity
        .get_document("MaybeGeneralization")
        .or_else(|_| entity.get_document("generalization"))
        .ok()
        .map(|value| string_any(value, &["Generalization", "generalization"]))
        .unwrap_or_default()
}

fn document_name(document: &Document) -> String {
    string_any(
        document,
        &["Name", "name", "UnqualifiedName", "unqualifiedName"],
    )
}

fn map_type<'a>(map: &'a [(&str, &str)], key: &str, fallback: &'a str) -> &'a str {
    map.iter()
        .find_map(|(candidate, value)| (*candidate == key).then_some(*value))
        .unwrap_or(fallback)
}

fn java_package(module: &str) -> String {
    module.to_lowercase()
}

fn java_proxy_name(qualified: &str) -> String {
    let (module, name) = qualified.split_once('.').unwrap_or((qualified, ""));
    format!("{}.proxies.{name}", java_package(module))
}

fn lower_camel(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn referenced(text: &str, needle: &str) -> bool {
    text.match_indices(needle).any(|(start, _)| {
        let before = text[..start].chars().next_back();
        let after = text[start + needle.len()..].chars().next();
        !before.is_some_and(is_java_word) && !after.is_some_and(is_java_word)
    })
}

fn is_java_word(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

fn unique_modules<'a>(units: impl Iterator<Item = &'a SourceUnit>) -> Vec<String> {
    let mut modules: Vec<_> = units.filter_map(|unit| unit.module_name.clone()).collect();
    modules.sort();
    modules.dedup();
    modules
}

fn collect_java_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    if !directory.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_java_files(&path, files)?;
        } else if path.extension().and_then(|value| value.to_str()) == Some("java") {
            files.push(path);
        }
    }
    Ok(())
}

fn write_missing(path: &Path, contents: &str) -> Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_bson::doc;

    #[test]
    fn reference_matching_observes_java_identifier_boundaries() {
        assert!(referenced(
            "demo.proxies.Record record;",
            "demo.proxies.Record"
        ));
        assert!(!referenced(
            "demo.proxies.RecordExtra record;",
            "demo.proxies.Record"
        ));
        assert!(!referenced(
            "Xdemo.proxies.Record record;",
            "demo.proxies.Record"
        ));
    }

    #[test]
    fn maps_microflow_types_and_results() {
        let boolean = doc! { "$Type": "DataTypes$BooleanType" };
        let object = doc! { "$Type": "DataTypes$ObjectType", "Entity": "Demo.Record" };
        assert_eq!(java_data_type(Some(&boolean), false), "java.lang.Boolean");
        assert_eq!(java_data_type(Some(&boolean), true), "boolean");
        assert_eq!(java_data_type(Some(&object), false), "demo.proxies.Record");
        assert!(microflow_result(Some(&object)).contains("Record.initialize"));
        assert_eq!(microflow_result(None), "return;");
        assert_eq!(lower_camel("RunFlow"), "runFlow");
    }
}
