//! Runtime web-operation catalog compilation.
//!
//! The catalog is the authorization and dispatch boundary used by generated
//! web data sources. It is built once from a project-wide index so page,
//! layout, snippet, menu, microflow, nanoflow, and domain references are
//! resolved consistently.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use mxrs_bson::{Bson, Document, doc};
use mxrs_compiler_support::{
    array_docs, get_doc_any, get_id_any, get_str_any, menu_operation_id, operation_id, string_list,
};
use mxrs_model::Project;

use crate::CompilerError;

#[derive(Debug, Clone)]
struct WebUnit {
    module_name: String,
    document: Document,
}

/// Compiles all Runtime operations reachable from web pages, layouts, menus,
/// snippets, and client-side nanoflows.
pub struct WebOperationCompiler {
    units: Vec<WebUnit>,
    document_index: HashMap<String, Document>,
    role_map: HashMap<String, Vec<String>>,
}

pub const DATA_GRID_WIDGET_ID: &str = "com.mendix.widget.web.datagrid.Datagrid";

impl WebOperationCompiler {
    /// Builds the cross-document index with one pass over the raw project
    /// units. Folder containment is followed back to the owning module.
    pub fn new(project: &Project) -> Result<Self, CompilerError> {
        let units = project.all_units()?;
        let parent_by_id: HashMap<_, _> = units
            .iter()
            .map(|unit| (unit.unit_id.clone(), unit.container_id.clone()))
            .collect();
        let module_name_by_id: HashMap<_, _> = project
            .modules()?
            .into_iter()
            .filter_map(|module| module.name.map(|name| (module.id, name)))
            .collect();
        let documents = units
            .iter()
            .map(|unit| {
                let document = project.mpr().parse_contents(unit)?;
                let module_name =
                    owning_module_name(&unit.container_id, &parent_by_id, &module_name_by_id)
                        .unwrap_or_default();
                Ok((module_name, document))
            })
            .collect::<mxrs_model::Result<Vec<_>>>()?;
        Ok(Self::from_documents(
            documents,
            mxrs_compiler_support::project_role_map(project)?,
        ))
    }

    /// In-memory constructor for integrations that already decoded project
    /// units. Each tuple is `(owning_module_name, document)`.
    pub fn from_documents(
        documents: impl IntoIterator<Item = (String, Document)>,
        role_map: HashMap<String, Vec<String>>,
    ) -> Self {
        let mut units: Vec<_> = documents
            .into_iter()
            .map(|(module_name, document)| WebUnit {
                module_name,
                document,
            })
            .collect();
        units.sort_by(|left, right| {
            (unit_type(&left.document), qualified_name(left))
                .cmp(&(unit_type(&right.document), qualified_name(right)))
        });
        let mut document_index = HashMap::new();
        for unit in &units {
            index_documents(&unit.document, &mut document_index);
        }
        Self {
            units,
            document_index,
            role_map,
        }
    }

    /// Returns a deterministic catalog, deduplicated by operation id while
    /// preserving project traversal order.
    pub fn compile(&self) -> Vec<Document> {
        let mut operations = Vec::new();
        for unit in self
            .units
            .iter()
            .filter(|unit| matches!(unit_type(&unit.document), "Forms$Page" | "Forms$Layout"))
        {
            operations.extend(self.page_operations(unit));
        }
        for unit in self
            .units
            .iter()
            .filter(|unit| unit_type(&unit.document) == "Microflows$Nanoflow")
        {
            operations.extend(self.nanoflow_operations(&qualified_name(unit), &unit.document));
        }
        deduplicate_operations(operations)
    }

    /// Writes the JSON array consumed by the generated web runtime.
    pub fn write(&self, path: impl AsRef<Path>) -> Result<Vec<Document>, CompilerError> {
        let operations = self.compile();
        let json = json_value(&Bson::Array(
            operations.iter().cloned().map(Bson::Document).collect(),
        ));
        std::fs::write(path, serde_json::to_vec(&json)?)?;
        Ok(operations)
    }

    fn page_operations(&self, unit: &WebUnit) -> Vec<Document> {
        let page_name = qualified_name(unit);
        let role_sets = self.allowed_role_sets(&unit.document);
        let mut documents = vec![unit.document.clone()];
        self.collect_snippets(&unit.document, &mut HashSet::new(), &mut documents);
        let mut operations = Vec::new();
        for document in documents {
            for widget_type in ["CustomWidgets$CustomWidget", "Forms$ListView"] {
                for widget in nested(&document, widget_type) {
                    if let Some(operation) =
                        self.list_operation(&page_name, &widget, &role_sets, &document)
                    {
                        operations.push(operation);
                    }
                }
            }
            for widget in nested(&document, "Forms$DataView") {
                if let Some(operation) = self.data_view_operation(&page_name, &widget, &role_sets) {
                    operations.push(operation);
                }
            }
            operations.extend(self.action_operations(&page_name, &document, &role_sets));
            operations.extend(self.menu_operations(&document));
            operations.extend(self.grid_filter_operations(&page_name, &document, &role_sets));
        }
        if self.popup_page(unit) {
            operations.push(operation_record(
                operation_id(&page_name, "$cancelChanges"),
                "rollback",
                doc! { "Objects": ["AnyObjectList"] },
                Document::new(),
                role_sets,
            ));
        }
        deduplicate_operations(operations)
    }

    fn collect_snippets(
        &self,
        document: &Document,
        seen: &mut HashSet<String>,
        result: &mut Vec<Document>,
    ) {
        for call in nested(document, "Forms$SnippetCallWidget") {
            let reference = get_doc_any(&call, &["FormCall"])
                .and_then(|form| get_str_any(&form, &["Form"]))
                .unwrap_or_default();
            if reference.is_empty() || !seen.insert(reference.clone()) {
                continue;
            }
            if let Some(unit) = self.qualified_unit("Forms$Snippet", &reference) {
                result.push(unit.document.clone());
                self.collect_snippets(&unit.document, seen, result);
            }
        }
    }

    fn list_operation(
        &self,
        page_name: &str,
        widget: &Document,
        role_sets: &[Vec<String>],
        page: &Document,
    ) -> Option<Document> {
        let source = ListDataSource::resolve(self, widget)?;
        let widget_name = get_str_any(widget, &["Name"]).unwrap_or_default();
        if source.nanoflow {
            return None;
        }
        if source.xpath {
            let parameters = object_page_parameters(page, &xpath_variables(&source.constraint))?;
            return Some(operation_record(
                operation_id(page_name, &widget_name),
                "retrieve",
                typed_parameters(parameters),
                doc! {
                    "PageName": page_name,
                    "WidgetName": format!("{page_name}.{widget_name}"),
                    "UsedAssociations": Bson::Array(vec![]),
                    "UsedAttributes": string_array(used_attributes(widget, &source.entity)),
                    "XPath": xpath(&source.entity, &source.constraint),
                },
                role_sets.to_vec(),
            ));
        }
        if !source.association_path.is_empty() {
            return self.association_operation(
                page_name,
                widget,
                role_sets,
                &source.association_path,
            );
        }
        if !source.microflow_name.is_empty() {
            return self.retrieve_by_microflow(
                page_name,
                widget,
                role_sets,
                &source.microflow_name,
            );
        }
        None
    }

    fn data_view_operation(
        &self,
        page_name: &str,
        widget: &Document,
        role_sets: &[Vec<String>],
    ) -> Option<Document> {
        let source = get_doc_any(widget, &["DataSource"])?;
        if unit_type(&source) == "Forms$MicroflowSource" {
            let name = flow_name(&source, "Microflow");
            return (!name.is_empty())
                .then(|| self.retrieve_by_microflow(page_name, widget, role_sets, &name))
                .flatten();
        }
        let path = entity_ref_path(get_doc_any(&source, &["EntityRef"]).as_ref());
        (!path.is_empty())
            .then(|| self.association_operation(page_name, widget, role_sets, &path))
            .flatten()
    }

    fn association_operation(
        &self,
        page_name: &str,
        widget: &Document,
        role_sets: &[Vec<String>],
        path: &str,
    ) -> Option<Document> {
        let source_entity = self.association_source_entity(path)?;
        let destination = path.rsplit('/').next().unwrap_or_default();
        let widget_name = get_str_any(widget, &["Name"]).unwrap_or_default();
        Some(operation_record(
            operation_id(page_name, &widget_name),
            "retrieve",
            doc! { "CurrentObject": [source_entity] },
            doc! {
                "PageName": page_name,
                "WidgetName": format!("{page_name}.{widget_name}"),
                "UsedAssociations": string_array(used_associations(widget, destination)),
                "UsedAttributes": string_array(used_attributes(widget, destination)),
                "EntityPath": path,
            },
            role_sets.to_vec(),
        ))
    }

    fn retrieve_by_microflow(
        &self,
        page_name: &str,
        widget: &Document,
        role_sets: &[Vec<String>],
        name: &str,
    ) -> Option<Document> {
        let parameters = self.microflow_parameters(name)?;
        let widget_name = get_str_any(widget, &["Name"]).unwrap_or_default();
        let entity = self.microflow_return_entity(name).unwrap_or_default();
        Some(operation_record(
            operation_id(page_name, &widget_name),
            "retrieveByMicroflow",
            typed_parameters(parameters),
            doc! {
                "MicroflowName": name,
                "PageName": page_name,
                "WidgetName": format!("{page_name}.{widget_name}"),
                "UsedAssociations": string_array(used_associations(widget, &entity)),
                "UsedAttributes": string_array(used_attributes(widget, &entity)),
            },
            self.microflow_role_sets(role_sets, name),
        ))
    }

    fn action_operations(
        &self,
        page_name: &str,
        document: &Document,
        role_sets: &[Vec<String>],
    ) -> Vec<Document> {
        let mut actions = Vec::new();
        for (widget_type, action_key) in [
            ("Forms$ActionButton", "Action"),
            ("Forms$DivContainer", "OnClickAction"),
        ] {
            for widget in nested(document, widget_type) {
                if let Some(action) = get_doc_any(&widget, &[action_key])
                    && let Some(operation) =
                        self.action_operation(page_name, &widget, &action, role_sets)
                {
                    actions.push(operation);
                }
            }
        }
        for widget in nested(document, "CustomWidgets$CustomWidget") {
            for value in nested(&widget, "CustomWidgets$WidgetValue") {
                if let Some(action) = get_doc_any(&value, &["Action"])
                    && let Some(operation) =
                        self.action_operation(page_name, &widget, &action, role_sets)
                {
                    actions.push(operation);
                }
            }
        }
        actions
    }

    fn action_operation(
        &self,
        page_name: &str,
        widget: &Document,
        action: &Document,
        role_sets: &[Vec<String>],
    ) -> Option<Document> {
        let widget_name = get_str_any(widget, &["Name"]).unwrap_or_default();
        let operation_type = match unit_type(action) {
            "Forms$SaveChangesClientAction" => Some("commit"),
            "Forms$CancelChangesClientAction" => Some("rollback"),
            "Forms$DeleteClientAction" => Some("delete"),
            "Forms$CreateObjectClientAction" => Some("create"),
            _ => None,
        };
        if let Some(operation_type) = operation_type {
            let (parameters, constants) = if operation_type == "create" {
                let entity = get_doc_any(action, &["EntityRef"])
                    .and_then(|reference| get_str_any(&reference, &["Entity"]))
                    .unwrap_or_default();
                (Document::new(), doc! { "ObjectType": entity })
            } else {
                (doc! { "Objects": ["AnyObjectList"] }, Document::new())
            };
            return Some(operation_record(
                operation_id(page_name, &widget_name),
                operation_type,
                parameters,
                constants,
                role_sets.to_vec(),
            ));
        }
        if unit_type(action) != "Forms$MicroflowAction" {
            return None;
        }
        let name = flow_name(action, "Microflow");
        let parameters = self.microflow_parameters(&name)?;
        Some(operation_record(
            operation_id(page_name, &widget_name),
            "callMicroflow",
            typed_parameters(parameters),
            doc! { "MicroflowName": name.clone() },
            self.microflow_role_sets(role_sets, &name),
        ))
    }

    fn menu_operations(&self, document: &Document) -> Vec<Document> {
        let mut result = Vec::new();
        for kind in [
            "Forms$NavigationTree",
            "Forms$MenuBar",
            "Forms$SimpleMenuBar",
        ] {
            for widget in nested(document, kind) {
                let Some(source) = get_doc_any(&widget, &["MenuSource"]) else {
                    continue;
                };
                for item in self.menu_items(&source) {
                    self.collect_menu_operations(&item, &mut result);
                }
            }
        }
        result
    }

    fn grid_filter_operations(
        &self,
        page_name: &str,
        document: &Document,
        role_sets: &[Vec<String>],
    ) -> Vec<Document> {
        let mut result = Vec::new();
        for grid in nested(document, "CustomWidgets$CustomWidget") {
            if self.custom_widget_id(&grid).as_deref() != Some(DATA_GRID_WIDGET_ID) {
                continue;
            }
            let grid_name = get_str_any(&grid, &["Name"]).unwrap_or_default();
            for (filter, endpoint, caption) in self.grid_filter_specs(&grid) {
                let filter_name = get_str_any(&filter, &["Name"]).unwrap_or_default();
                result.push(operation_record(
                    operation_id(page_name, &format!("{grid_name}${filter_name}")),
                    "retrieve",
                    Document::new(),
                    doc! {
                        "PageName": page_name,
                        "WidgetName": format!("{page_name}.{grid_name}"),
                        "UsedAssociations": Bson::Array(vec![]),
                        "UsedAttributes": [format!("{endpoint}/{endpoint}.{caption}")],
                        "XPath": format!("//{endpoint}"),
                    },
                    role_sets.to_vec(),
                ));
            }
        }
        result
    }

    fn custom_widget_id(&self, widget: &Document) -> Option<String> {
        let pointer = get_doc_any(widget, &["Object"])
            .and_then(|object| get_id_any(&object, &["TypePointer"]))?;
        self.document_index.values().find_map(|document| {
            let object_type = get_doc_any(document, &["ObjectType"])?;
            (get_id_any(&object_type, &["$ID"]).as_deref() == Some(&pointer))
                .then(|| get_str_any(document, &["WidgetId"]))
                .flatten()
        })
    }

    fn custom_property_values(&self, object: &Document) -> HashMap<String, (String, Bson)> {
        array_docs(object, &["Properties"])
            .into_iter()
            .filter_map(|property| {
                let type_id = get_id_any(&property, &["TypePointer"])?;
                let property_type = self.document_index.get(&type_id)?;
                let key = get_str_any(property_type, &["PropertyKey"])?;
                let kind = get_doc_any(property_type, &["ValueType"])
                    .and_then(|value| get_str_any(&value, &["Type"]))
                    .unwrap_or_default();
                Some((
                    key,
                    (kind, property.get("Value").cloned().unwrap_or(Bson::Null)),
                ))
            })
            .collect()
    }

    fn grid_filter_specs(&self, grid: &Document) -> Vec<(Document, String, String)> {
        let Some(object) = get_doc_any(grid, &["Object"]) else {
            return Vec::new();
        };
        let values = self.custom_property_values(&object);
        let Some(columns) = values
            .get("columns")
            .and_then(|(_, value)| value.as_document())
        else {
            return Vec::new();
        };
        array_docs(columns, &["Objects"])
            .into_iter()
            .filter_map(|column| {
                let values = self.custom_property_values(&column);
                let attribute = values
                    .get("attribute")
                    .and_then(|(_, value)| value.as_document())?
                    .get_document("AttributeRef")
                    .ok()?;
                let steps = attribute
                    .get_document("EntityRef")
                    .ok()
                    .map(|reference| array_docs(reference, &["Steps"]))
                    .unwrap_or_default();
                let endpoint = steps
                    .last()
                    .and_then(|step| get_str_any(step, &["DestinationEntity"]))?;
                let caption = attribute
                    .get_str("Attribute")
                    .ok()?
                    .rsplit('.')
                    .next()?
                    .to_string();
                let filter = values
                    .get("filter")
                    .and_then(|(_, value)| value.as_document())
                    .map(|value| array_docs(value, &["Widgets"]))?
                    .into_iter()
                    .next()?;
                (!endpoint.is_empty() && !caption.is_empty()).then_some((filter, endpoint, caption))
            })
            .collect()
    }

    fn menu_items(&self, source: &Document) -> Vec<Document> {
        match unit_type(source) {
            "Forms$MenuDocumentSource" => get_str_any(source, &["Menu"])
                .and_then(|name| self.qualified_unit("Menus$MenuDocument", &name))
                .and_then(|unit| get_doc_any(&unit.document, &["ItemCollection"]))
                .map(|collection| array_docs(&collection, &["Items"]))
                .unwrap_or_default(),
            "Forms$NavigationSource" => {
                let profile_name = get_str_any(source, &["NavigationProfile"]).unwrap_or_default();
                self.units
                    .iter()
                    .filter(|unit| unit_type(&unit.document) == "Navigation$NavigationDocument")
                    .flat_map(|unit| array_docs(&unit.document, &["Profiles"]))
                    .find(|profile| {
                        get_str_any(profile, &["Name"]).as_deref() == Some(&profile_name)
                    })
                    .and_then(|profile| get_doc_any(&profile, &["Menu"]))
                    .map(|menu| array_docs(&menu, &["Items"]))
                    .unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }

    fn collect_menu_operations(&self, item: &Document, result: &mut Vec<Document>) {
        if let Some(action) = get_doc_any(item, &["Action"])
            && unit_type(&action) == "Forms$MicroflowAction"
        {
            let name = flow_name(&action, "Microflow");
            if let Some(parameters) = self.microflow_parameters(&name) {
                result.push(operation_record(
                    menu_operation_id(&action),
                    "callMicroflow",
                    typed_parameters(parameters),
                    doc! { "MicroflowName": name.clone() },
                    self.microflow_role_sets(&[], &name),
                ));
            }
        }
        for child in array_docs(item, &["Items"]) {
            self.collect_menu_operations(&child, result);
        }
    }

    fn nanoflow_operations(&self, qualified_name: &str, document: &Document) -> Vec<Document> {
        let mut entities = HashMap::new();
        for parameter in nested(document, "Microflows$MicroflowParameter") {
            if let (Some(name), Some(entity)) = (
                get_str_any(&parameter, &["Name"]),
                get_doc_any(&parameter, &["VariableType"])
                    .and_then(|value| get_str_any(&value, &["Entity"])),
            ) && !entity.is_empty()
            {
                entities.insert(name, entity);
            }
        }
        for activity in nested(document, "Microflows$ActionActivity") {
            if let Some(action) = get_doc_any(&activity, &["Action"])
                && let Some(entity) = get_str_any(&action, &["Entity"])
                && let Some(variable) =
                    get_str_any(&action, &["OutputVariableName", "VariableName"])
                && !entity.is_empty()
            {
                entities.insert(variable, entity);
            }
        }
        nested(document, "Microflows$ActionActivity")
            .into_iter()
            .filter_map(|activity| {
                let action = get_doc_any(&activity, &["Action"])?;
                let id = get_id_any(&activity, &["$ID"]).unwrap_or_default();
                let operation_id = operation_id(qualified_name, &id);
                if unit_type(&action) == "Microflows$MicroflowCallAction" {
                    let name = get_doc_any(&action, &["MicroflowCall"])
                        .and_then(|call| get_str_any(&call, &["Microflow"]))?;
                    let parameters = self.microflow_parameters(&name)?;
                    return Some(operation_record(
                        operation_id,
                        "callMicroflow",
                        typed_parameters(parameters),
                        doc! { "MicroflowName": name.clone() },
                        self.microflow_role_sets(&[], &name),
                    ));
                }
                if !commit_action(&action) {
                    return None;
                }
                let variable = get_str_any(
                    &action,
                    &[
                        "CommitVariableName",
                        "ChangeVariableName",
                        "OutputVariableName",
                        "VariableName",
                    ],
                )
                .unwrap_or_default();
                let parameter = entities
                    .get(&variable)
                    .map(|entity| format!("[{entity}]"))
                    .unwrap_or_else(|| "AnyObjectList".to_string());
                Some(operation_record(
                    operation_id,
                    "commit",
                    doc! { "Objects": [parameter] },
                    Document::new(),
                    vec![],
                ))
            })
            .collect()
    }

    fn microflow_parameters(&self, name: &str) -> Option<BTreeMap<String, String>> {
        if name == "System.ShowHomePage"
            && self.qualified_unit("Microflows$Microflow", name).is_none()
        {
            return Some(BTreeMap::new());
        }
        let flow = self.qualified_unit("Microflows$Microflow", name)?;
        let mut parameters = BTreeMap::new();
        for parameter in nested(&flow.document, "Microflows$MicroflowParameter") {
            let variable_type = get_doc_any(&parameter, &["VariableType"])?;
            if unit_type(&variable_type) != "DataTypes$ObjectType" {
                return None;
            }
            let entity = get_str_any(&variable_type, &["Entity"])?;
            if entity.is_empty() {
                return None;
            }
            parameters.insert(
                get_str_any(&parameter, &["Name"]).unwrap_or_default(),
                entity,
            );
        }
        Some(parameters)
    }

    fn microflow_return_entity(&self, name: &str) -> Option<String> {
        self.qualified_unit("Microflows$Microflow", name)
            .and_then(|unit| get_doc_any(&unit.document, &["MicroflowReturnType"]))
            .and_then(|value| get_str_any(&value, &["Entity"]))
    }

    fn connector_fallback(&self, microflow_name: &str, entity: &str) -> bool {
        let Some(flow) = self.qualified_unit("Microflows$Microflow", microflow_name) else {
            return false;
        };
        let actions = nested(
            &flow.document,
            "DatabaseConnector$ExecuteDatabaseQueryAction",
        );
        let [action] = actions.as_slice() else {
            return false;
        };
        let Some(query_name) = get_str_any(action, &["Query"]) else {
            return false;
        };
        let Some((connection_name, query_name)) = query_name.rsplit_once('.') else {
            return false;
        };
        let Some(connection) =
            self.qualified_unit("DatabaseConnector$DatabaseConnection", connection_name)
        else {
            return false;
        };
        if !["ConnectionString", "UserName", "Password"]
            .iter()
            .all(|field| {
                let Some(constant_name) = get_str_any(&connection.document, &[*field]) else {
                    return false;
                };
                !constant_name.is_empty()
                    && self
                        .qualified_unit("Constants$Constant", &constant_name)
                        .is_some_and(|constant| {
                            get_str_any(&constant.document, &["DefaultValue"])
                                .unwrap_or_default()
                                .is_empty()
                        })
            })
        {
            return false;
        }
        let Some(query) = array_docs(&connection.document, &["Queries"])
            .into_iter()
            .find(|query| get_str_any(query, &["Name"]).as_deref() == Some(query_name))
        else {
            return false;
        };
        if !get_str_any(&query, &["Query"]).is_some_and(|sql| simple_select_query(&sql)) {
            return false;
        }
        let mappings = array_docs(&query, &["TableMappings"]);
        mappings.len() == 1 && get_str_any(&mappings[0], &["Entity"]).as_deref() == Some(entity)
    }

    fn association_source_entity(&self, path: &str) -> Option<String> {
        let association_name = path.split('/').next()?;
        let (module_name, name) = association_name.rsplit_once('.')?;
        let domain = self.units.iter().find(|unit| {
            unit.module_name == module_name
                && unit_type(&unit.document) == "DomainModels$DomainModel"
        })?;
        let association = array_docs(&domain.document, &["Associations"])
            .into_iter()
            .find(|value| get_str_any(value, &["Name"]).as_deref() == Some(name))?;
        let parent_id = get_id_any(&association, &["ParentPointer"])?;
        let entity = array_docs(&domain.document, &["Entities"])
            .into_iter()
            .find(|value| get_id_any(value, &["$ID"]).as_deref() == Some(&parent_id))?;
        get_str_any(&entity, &["Name"]).map(|name| format!("{module_name}.{name}"))
    }

    fn allowed_role_sets(&self, document: &Document) -> Vec<Vec<String>> {
        let roles = string_list(document, &["AllowedModuleRoles"]);
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        for role in roles {
            for user_role in self.role_map.get(&role).into_iter().flatten() {
                if seen.insert(user_role.clone()) {
                    result.push(vec![user_role.clone()]);
                }
            }
        }
        result
    }

    fn microflow_role_sets(&self, container: &[Vec<String>], name: &str) -> Vec<Vec<String>> {
        let flow_roles = self
            .qualified_unit("Microflows$Microflow", name)
            .map(|unit| self.allowed_role_sets(&unit.document))
            .unwrap_or_default();
        if flow_roles.is_empty() {
            return container.to_vec();
        }
        if container.is_empty() {
            return flow_roles;
        }
        container
            .iter()
            .filter(|role| flow_roles.contains(role))
            .cloned()
            .collect()
    }

    fn popup_page(&self, page: &WebUnit) -> bool {
        if unit_type(&page.document) != "Forms$Page" {
            return false;
        }
        let layout_name = get_doc_any(&page.document, &["FormCall"])
            .and_then(|call| get_str_any(&call, &["Form"]))
            .unwrap_or_default();
        let Some(layout) = self.qualified_unit("Forms$Layout", &layout_name) else {
            return false;
        };
        let name = get_str_any(&layout.document, &["Name"]).unwrap_or_default();
        if name.to_ascii_lowercase().contains("popup") {
            return true;
        }
        let width = bson_i32(layout.document.get("CanvasWidth"));
        width > 0
            && width <= 800
            && nested(&layout.document, "Forms$NavigationTree").is_empty()
            && nested(&layout.document, "Forms$MenuBar").is_empty()
            && nested(&layout.document, "Forms$SimpleMenuBar").is_empty()
            && nested(&layout.document, "Forms$Header").is_empty()
    }

    fn qualified_unit(&self, type_name: &str, name: &str) -> Option<&WebUnit> {
        self.units
            .iter()
            .find(|unit| unit_type(&unit.document) == type_name && qualified_name(unit) == name)
    }
}

struct ListDataSource {
    xpath: bool,
    nanoflow: bool,
    entity: String,
    constraint: String,
    association_path: String,
    microflow_name: String,
}

impl ListDataSource {
    fn resolve(compiler: &WebOperationCompiler, widget: &Document) -> Option<Self> {
        let direct = get_doc_any(widget, &["DataSource"]);
        let xpath_source = direct
            .as_ref()
            .filter(|source| database_source(source))
            .cloned()
            .or_else(|| {
                nested(widget, "CustomWidgets$CustomWidgetXPathSource")
                    .into_iter()
                    .next()
            });
        let association = direct
            .as_ref()
            .filter(|source| unit_type(source) == "Forms$AssociationSource")
            .cloned()
            .or_else(|| nested(widget, "Forms$AssociationSource").into_iter().next());
        let microflow = direct
            .as_ref()
            .filter(|source| unit_type(source) == "Forms$MicroflowSource")
            .cloned()
            .or_else(|| nested(widget, "Forms$MicroflowSource").into_iter().next());
        let nanoflow = direct
            .as_ref()
            .filter(|source| unit_type(source) == "Forms$NanoflowSource")
            .cloned()
            .or_else(|| nested(widget, "Forms$NanoflowSource").into_iter().next());
        let microflow_name = microflow
            .as_ref()
            .map(|source| flow_name(source, "Microflow"))
            .unwrap_or_default();
        let nanoflow_name = nanoflow
            .as_ref()
            .and_then(|source| get_str_any(source, &["Nanoflow"]))
            .unwrap_or_default();
        let association_path = association
            .as_ref()
            .map(|source| entity_ref_path(get_doc_any(source, &["EntityRef"]).as_ref()))
            .unwrap_or_default();
        let entity = xpath_source
            .as_ref()
            .and_then(source_entity)
            .or_else(|| association.as_ref().and_then(source_entity))
            .or_else(|| compiler.microflow_return_entity(&microflow_name))
            .or_else(|| {
                compiler
                    .qualified_unit("Microflows$Nanoflow", &nanoflow_name)
                    .and_then(|unit| get_doc_any(&unit.document, &["MicroflowReturnType"]))
                    .and_then(|value| get_str_any(&value, &["Entity"]))
            })
            .unwrap_or_default();
        let connector_fallback = compiler.connector_fallback(&microflow_name, &entity);
        let result = Self {
            xpath: xpath_source.is_some() || connector_fallback,
            nanoflow: nanoflow.is_some() && !nanoflow_name.is_empty(),
            entity,
            constraint: xpath_source
                .as_ref()
                .and_then(|source| get_str_any(source, &["XPathConstraint"]))
                .unwrap_or_default(),
            association_path,
            microflow_name,
        };
        (result.xpath
            || result.nanoflow
            || !result.association_path.is_empty()
            || compiler
                .qualified_unit("Microflows$Microflow", &result.microflow_name)
                .is_some())
        .then_some(result)
    }
}

fn operation_record(
    id: String,
    operation_type: &str,
    parameters: Document,
    constants: Document,
    role_sets: Vec<Vec<String>>,
) -> Document {
    doc! {
        "operationId": id,
        "operationType": operation_type,
        "parameters": parameters,
        "constants": constants,
        "allowedUserRoleSets": Bson::Array(role_sets.into_iter().map(string_array).collect()),
    }
}

fn typed_parameters(parameters: BTreeMap<String, String>) -> Document {
    parameters
        .into_iter()
        .map(|(name, value)| (name, string_array([value])))
        .collect()
}

fn string_array(values: impl IntoIterator<Item = String>) -> Bson {
    Bson::Array(values.into_iter().map(Bson::String).collect())
}

fn json_value(value: &Bson) -> serde_json::Value {
    match value {
        Bson::Double(value) => serde_json::json!(value),
        Bson::String(value) => serde_json::Value::String(value.clone()),
        Bson::Array(values) => serde_json::Value::Array(values.iter().map(json_value).collect()),
        Bson::Document(document) => serde_json::Value::Object(
            document
                .iter()
                .map(|(key, value)| (key.clone(), json_value(value)))
                .collect(),
        ),
        Bson::Boolean(value) => serde_json::Value::Bool(*value),
        Bson::Int32(value) => serde_json::json!(value),
        Bson::Int64(value) => serde_json::json!(value),
        Bson::Null => serde_json::Value::Null,
        other => serde_json::Value::String(other.to_string()),
    }
}

fn xpath(entity: &str, constraint: &str) -> String {
    let predicate = constraint.trim();
    if predicate.is_empty() {
        format!("//{entity}")
    } else if predicate.starts_with('[') {
        format!("//{entity}{predicate}")
    } else {
        format!("//{entity}[{predicate}]")
    }
}

pub(crate) fn xpath_variables(constraint: &str) -> Vec<String> {
    let bytes = constraint.as_bytes();
    let mut result = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'$' && index + 1 < bytes.len() && is_ident_start(bytes[index + 1]) {
            let start = index + 1;
            index = start + 1;
            while index < bytes.len() && is_ident_continue(bytes[index]) {
                index += 1;
            }
            let name = constraint[start..index].to_string();
            if !result.contains(&name) {
                result.push(name);
            }
        } else {
            index += 1;
        }
    }
    result
}

pub(crate) fn object_page_parameters(
    page: &Document,
    names: &[String],
) -> Option<BTreeMap<String, String>> {
    let types: HashMap<_, _> = array_docs(page, &["Parameters"])
        .into_iter()
        .filter_map(|parameter| {
            Some((
                get_str_any(&parameter, &["Name"])?,
                get_doc_any(&parameter, &["ParameterType"])?,
            ))
        })
        .collect();
    names
        .iter()
        .map(|name| {
            let parameter_type = types.get(name)?;
            (unit_type(parameter_type) == "DataTypes$ObjectType")
                .then(|| get_str_any(parameter_type, &["Entity"]))
                .flatten()
                .filter(|entity| !entity.is_empty())
                .map(|entity| (name.clone(), entity))
        })
        .collect()
}

fn used_attributes(value: &Document, entity: &str) -> Vec<String> {
    let mut result = Vec::new();
    collect_attribute_names(&Bson::Document(value.clone()), entity, &mut result);
    result.sort();
    result.dedup();
    result
}

fn used_associations(value: &Document, entity: &str) -> Vec<String> {
    let mut result: Vec<_> = used_attributes(value, entity)
        .into_iter()
        .filter_map(|path| {
            let parts: Vec<_> = path.split('/').collect();
            (parts.len() > 2).then(|| parts[..parts.len() - 1].join("/"))
        })
        .collect();
    result.sort();
    result.dedup();
    result
}

fn collect_attribute_names(value: &Bson, entity: &str, result: &mut Vec<String>) {
    match value {
        Bson::Document(document) => {
            if let Some(attribute) = get_str_any(document, &["Attribute"]) {
                let steps = get_doc_any(document, &["EntityRef"])
                    .map(|reference| array_docs(&reference, &["Steps"]))
                    .unwrap_or_default();
                if steps.is_empty() && attribute.starts_with(&format!("{entity}.")) {
                    result.push(format!("{entity}/{attribute}"));
                } else if !steps.is_empty()
                    && steps.iter().all(|step| {
                        get_str_any(step, &["Association"]).is_some()
                            && get_str_any(step, &["DestinationEntity"]).is_some()
                    })
                    && attribute.contains('.')
                {
                    let mut parts = vec![entity.to_string()];
                    for step in steps {
                        parts.push(get_str_any(&step, &["Association"]).unwrap_or_default());
                        parts.push(get_str_any(&step, &["DestinationEntity"]).unwrap_or_default());
                    }
                    parts.push(attribute);
                    result.push(parts.join("/"));
                }
            }
            for child in document.values() {
                collect_attribute_names(child, entity, result);
            }
        }
        Bson::Array(items) => {
            for child in mxrs_bson::parse_array(Some(items)).items {
                collect_attribute_names(&child, entity, result);
            }
        }
        Bson::String(text) => collect_current_object_attributes(text, entity, result),
        _ => {}
    }
}

fn collect_current_object_attributes(text: &str, entity: &str, result: &mut Vec<String>) {
    let marker = "$currentObject/";
    let mut rest = text;
    while let Some(offset) = rest.find(marker) {
        rest = &rest[offset + marker.len()..];
        let end = rest
            .bytes()
            .position(|byte| !is_ident_continue(byte))
            .unwrap_or(rest.len());
        if end > 0 {
            let attribute = &rest[..end];
            result.push(format!("{entity}/{entity}.{attribute}"));
        }
        rest = &rest[end..];
    }
}

fn nested(value: &Document, type_name: &str) -> Vec<Document> {
    let mut result = Vec::new();
    collect_nested(&Bson::Document(value.clone()), type_name, &mut result);
    result
}

fn collect_nested(value: &Bson, type_name: &str, result: &mut Vec<Document>) {
    match value {
        Bson::Document(document) => {
            if unit_type(document) == type_name {
                result.push(document.clone());
            }
            for child in document.values() {
                collect_nested(child, type_name, result);
            }
        }
        Bson::Array(items) => {
            for child in mxrs_bson::parse_array(Some(items)).items {
                collect_nested(&child, type_name, result);
            }
        }
        _ => {}
    }
}

fn source_entity(source: &Document) -> Option<String> {
    let reference = get_doc_any(source, &["EntityRef"])?;
    let steps = array_docs(&reference, &["Steps"]);
    steps
        .last()
        .and_then(|step| get_str_any(step, &["DestinationEntity"]))
        .or_else(|| get_str_any(&reference, &["Entity"]))
}

fn entity_ref_path(reference: Option<&Document>) -> String {
    let Some(reference) = reference else {
        return String::new();
    };
    array_docs(reference, &["Steps"])
        .into_iter()
        .flat_map(|step| {
            [
                get_str_any(&step, &["Association"]).unwrap_or_default(),
                get_str_any(&step, &["DestinationEntity"]).unwrap_or_default(),
            ]
        })
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

fn flow_name(source: &Document, kind: &str) -> String {
    get_doc_any(source, &[&format!("{kind}Settings")])
        .and_then(|settings| get_str_any(&settings, &[kind]))
        .filter(|name| !name.is_empty())
        .or_else(|| get_str_any(source, &[kind]))
        .unwrap_or_default()
}

fn database_source(source: &Document) -> bool {
    matches!(
        unit_type(source),
        "Forms$DatabaseSource" | "Forms$NewListViewDatabaseSource" | "Forms$ListViewXPathSource"
    )
}

fn simple_select_query(sql: &str) -> bool {
    let sql = sql.trim().trim_end_matches(';').trim();
    let lower = sql.to_ascii_lowercase();
    let Some(body) = lower.strip_prefix("select ") else {
        return false;
    };
    let Some((projection, table)) = body.rsplit_once(" from ") else {
        return false;
    };
    !projection.trim().is_empty()
        && !table.is_empty()
        && table
            .bytes()
            .enumerate()
            .all(|(index, byte)| is_ident_continue(byte) && (index > 0 || is_ident_start(byte)))
}

fn commit_action(action: &Document) -> bool {
    unit_type(action) == "Microflows$CommitAction"
        || matches!(
            unit_type(action),
            "Microflows$ChangeAction" | "Microflows$CreateChangeAction"
        ) && get_str_any(action, &["Commit"]).as_deref() == Some("Yes")
}

fn unit_type(document: &Document) -> &str {
    document.get_str("$Type").unwrap_or_default()
}

fn qualified_name(unit: &WebUnit) -> String {
    let name = get_str_any(&unit.document, &["Name"]).unwrap_or_default();
    if unit.module_name.is_empty() {
        name
    } else {
        format!("{}.{name}", unit.module_name)
    }
}

fn owning_module_name(
    container_id: &str,
    parent_by_id: &HashMap<String, String>,
    module_name_by_id: &HashMap<String, String>,
) -> Option<String> {
    let mut current = container_id;
    for _ in 0..64 {
        if let Some(name) = module_name_by_id.get(current) {
            return Some(name.clone());
        }
        let parent = parent_by_id.get(current)?;
        if parent == current {
            return None;
        }
        current = parent;
    }
    None
}

fn index_documents(value: &Document, index: &mut HashMap<String, Document>) {
    if let Some(id) = get_id_any(value, &["$ID"]) {
        index.insert(id, value.clone());
    }
    for child in value.values() {
        match child {
            Bson::Document(document) => index_documents(document, index),
            Bson::Array(items) => {
                for item in mxrs_bson::parse_array(Some(items)).items {
                    if let Bson::Document(document) = item {
                        index_documents(&document, index);
                    }
                }
            }
            _ => {}
        }
    }
}

fn deduplicate_operations(operations: Vec<Document>) -> Vec<Document> {
    let mut seen = HashSet::new();
    operations
        .into_iter()
        .filter(|operation| {
            operation
                .get_str("operationId")
                .ok()
                .is_some_and(|id| seen.insert(id.to_string()))
        })
        .collect()
}

fn bson_i32(value: Option<&Bson>) -> i32 {
    match value {
        Some(Bson::Int32(value)) => *value,
        Some(Bson::Int64(value)) => i32::try_from(*value).unwrap_or_default(),
        Some(Bson::Double(value)) => *value as i32,
        Some(Bson::String(value)) => value.parse().unwrap_or_default(),
        _ => 0,
    }
}

fn is_ident_start(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphabetic()
}

fn is_ident_continue(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compiler(documents: Vec<(&str, Document)>) -> WebOperationCompiler {
        WebOperationCompiler::from_documents(
            documents
                .into_iter()
                .map(|(module, document)| (module.to_string(), document)),
            HashMap::new(),
        )
    }

    #[test]
    fn operation_id_matches_the_ruby_sha256_base64_contract() {
        assert_eq!(
            operation_id("Sales.MyPage", "widget1"),
            "Xt1yRHfyEC1EqtmPr5GL8Q"
        );
    }

    #[test]
    fn compiles_xpath_and_microflow_operations_and_skips_nanoflow_sources() {
        let page = doc! {
            "$Type": "Forms$Page", "Name": "Home",
            "Parameters": mxrs_bson::build_array(vec![Bson::Document(doc! {
                "$Type": "Forms$PageParameter", "Name": "Parent",
                "ParameterType": { "$Type": "DataTypes$ObjectType", "Entity": "Demo.Parent" },
            })], 2),
            "Widgets": mxrs_bson::build_array(vec![
                Bson::Document(doc! {
                    "$Type": "CustomWidgets$CustomWidget", "Name": "grid",
                    "Object": { "DataSource": {
                        "$Type": "CustomWidgets$CustomWidgetXPathSource",
                        "EntityRef": { "Entity": "Demo.Item" },
                        "XPathConstraint": "[Demo.Item_Parent = $Parent]",
                    }},
                    "Column": { "Attribute": "Demo.Item.Name" },
                }),
                Bson::Document(doc! {
                    "$Type": "Forms$ListView", "Name": "loaded",
                    "DataSource": { "$Type": "Forms$MicroflowSource", "MicroflowSettings": { "Microflow": "Demo.Load" } },
                }),
                Bson::Document(doc! {
                    "$Type": "Forms$ListView", "Name": "client",
                    "DataSource": { "$Type": "Forms$NanoflowSource", "Nanoflow": "Demo.ClientLoad" },
                }),
            ], 2),
        };
        let load = doc! {
            "$Type": "Microflows$Microflow", "Name": "Load",
            "MicroflowReturnType": { "$Type": "DataTypes$ListType", "Entity": "Demo.Item" },
        };
        let client = doc! {
            "$Type": "Microflows$Nanoflow", "Name": "ClientLoad",
            "MicroflowReturnType": { "$Type": "DataTypes$ListType", "Entity": "Demo.Item" },
        };
        let operations = compiler(vec![("Demo", page), ("Demo", load), ("Demo", client)]).compile();
        assert_eq!(operations.len(), 2);
        let xpath = operations
            .iter()
            .find(|operation| operation.get_str("operationType").unwrap() == "retrieve")
            .unwrap();
        assert_eq!(
            xpath
                .get_document("constants")
                .unwrap()
                .get_str("XPath")
                .unwrap(),
            "//Demo.Item[Demo.Item_Parent = $Parent]"
        );
        assert_eq!(
            xpath
                .get_document("parameters")
                .unwrap()
                .get_array("Parent")
                .unwrap(),
            &vec![Bson::String("Demo.Parent".into())]
        );
        assert!(operations.iter().any(|operation| {
            operation.get_str("operationType").unwrap() == "retrieveByMicroflow"
        }));
    }

    #[test]
    fn compiles_native_actions_popup_rollback_and_server_nanoflow_calls() {
        let layout = doc! { "$Type": "Forms$Layout", "Name": "DialogPopup" };
        let page = doc! {
            "$Type": "Forms$Page", "Name": "Edit",
            "FormCall": { "Form": "Demo.DialogPopup" },
            "Button": { "$Type": "Forms$ActionButton", "Name": "save", "Action": { "$Type": "Forms$SaveChangesClientAction" } },
        };
        let flow = doc! { "$Type": "Microflows$Microflow", "Name": "Run" };
        let nano = doc! {
            "$Type": "Microflows$Nanoflow", "Name": "ClientRun",
            "ObjectCollection": { "Objects": mxrs_bson::build_array(vec![Bson::Document(doc! {
                "$ID": "11111111-1111-4111-8111-111111111111",
                "$Type": "Microflows$ActionActivity",
                "Action": { "$Type": "Microflows$MicroflowCallAction", "MicroflowCall": { "Microflow": "Demo.Run" } },
            })], 2) },
        };
        let operations = compiler(vec![
            ("Demo", layout),
            ("Demo", page),
            ("Demo", flow),
            ("Demo", nano),
        ])
        .compile();
        let kinds: Vec<_> = operations
            .iter()
            .map(|operation| operation.get_str("operationType").unwrap())
            .collect();
        assert!(kinds.contains(&"commit"));
        assert!(kinds.contains(&"rollback"));
        assert!(kinds.contains(&"callMicroflow"));
    }

    #[test]
    fn inventories_current_object_and_association_attributes() {
        let widget = doc! {
            "AttributeRef": { "Attribute": "Demo.Item.Name" },
            "Visibility": "$currentObject/Active = true",
            "Related": {
                "Attribute": "Demo.Parent.Name",
                "EntityRef": { "Steps": mxrs_bson::build_array(vec![Bson::Document(doc! {
                    "Association": "Demo.Item_Parent", "DestinationEntity": "Demo.Parent",
                })], 2) },
            },
        };
        assert_eq!(
            used_attributes(&widget, "Demo.Item"),
            vec![
                "Demo.Item/Demo.Item.Active",
                "Demo.Item/Demo.Item.Name",
                "Demo.Item/Demo.Item_Parent/Demo.Parent/Demo.Parent.Name",
            ]
        );
    }

    #[test]
    fn menu_id_falls_back_to_the_microflow_reference() {
        let action = doc! { "MicroflowSettings": { "Microflow": "Sales.ACT_Run" } };
        assert_eq!(
            menu_operation_id(&action),
            operation_id("Navigation", "Sales.ACT_Run")
        );
    }

    #[test]
    fn resolves_data_grid_schema_and_compiles_filter_operations() {
        let property_type = |id: &str, key: &str, kind: &str| {
            doc! {
                "$ID": id,
                "$Type": "CustomWidgets$WidgetPropertyType",
                "PropertyKey": key,
                "ValueType": { "Type": kind },
            }
        };
        let schema = doc! {
            "$ID": "schema",
            "$Type": "CustomWidgets$CustomWidgetType",
            "WidgetId": DATA_GRID_WIDGET_ID,
            "ObjectType": { "$ID": "grid-object-type" },
            "PropertyTypes": mxrs_bson::build_array(vec![
                Bson::Document(property_type("columns-type", "columns", "Object")),
                Bson::Document(property_type("attribute-type", "attribute", "Attribute")),
                Bson::Document(property_type("filter-type", "filter", "Widgets")),
            ], 2),
        };
        let property = |type_id: &str, value: Document| {
            Bson::Document(doc! { "TypePointer": type_id, "Value": value })
        };
        let column = doc! {
            "Properties": mxrs_bson::build_array(vec![
                property("attribute-type", doc! {
                    "AttributeRef": {
                        "Attribute": "Demo.Category.Name",
                        "EntityRef": { "Steps": mxrs_bson::build_array(vec![Bson::Document(doc! {
                            "DestinationEntity": "Demo.Category",
                        })], 2) },
                    },
                }),
                property("filter-type", doc! {
                    "Widgets": mxrs_bson::build_array(vec![Bson::Document(doc! {
                        "Name": "category",
                    })], 2),
                }),
            ], 2),
        };
        let page = doc! {
            "$Type": "Forms$Page", "Name": "Home",
            "Grid": {
                "$Type": "CustomWidgets$CustomWidget", "Name": "grid",
                "Object": {
                    "TypePointer": "grid-object-type",
                    "Properties": mxrs_bson::build_array(vec![property("columns-type", doc! {
                        "Objects": mxrs_bson::build_array(vec![Bson::Document(column)], 2),
                    })], 2),
                },
            },
        };
        let operations = compiler(vec![("", schema), ("Demo", page)]).compile();
        assert_eq!(operations.len(), 1);
        let constants = operations[0].get_document("constants").unwrap();
        assert_eq!(constants.get_str("XPath").unwrap(), "//Demo.Category");
        assert_eq!(
            operations[0].get_str("operationId").unwrap(),
            operation_id("Demo.Home", "grid$category")
        );
    }

    #[test]
    fn lowers_a_simple_unconfigured_connector_read_to_xpath() {
        let flow = doc! {
            "$Type": "Microflows$Microflow", "Name": "Load",
            "MicroflowReturnType": { "Entity": "Demo.Item" },
            "Action": {
                "$Type": "DatabaseConnector$ExecuteDatabaseQueryAction",
                "Query": "Demo.Database.SelectItems",
            },
        };
        let connection = doc! {
            "$Type": "DatabaseConnector$DatabaseConnection", "Name": "Database",
            "ConnectionString": "Demo.Source", "UserName": "Demo.User", "Password": "Demo.Password",
            "Queries": mxrs_bson::build_array(vec![Bson::Document(doc! {
                "Name": "SelectItems", "Query": "SELECT name FROM items;",
                "TableMappings": mxrs_bson::build_array(vec![Bson::Document(doc! {
                    "Entity": "Demo.Item",
                })], 2),
            })], 2),
        };
        let constants = ["Source", "User", "Password"].map(|name| {
            (
                "Demo",
                doc! { "$Type": "Constants$Constant", "Name": name, "DefaultValue": "" },
            )
        });
        let page = doc! {
            "$Type": "Forms$Page", "Name": "Home",
            "List": {
                "$Type": "Forms$ListView", "Name": "items",
                "DataSource": {
                    "$Type": "Forms$MicroflowSource",
                    "MicroflowSettings": { "Microflow": "Demo.Load" },
                },
            },
        };
        let mut documents = vec![("Demo", page), ("Demo", flow), ("Demo", connection)];
        documents.extend(constants);
        let operations = compiler(documents).compile();
        assert_eq!(operations.len(), 1);
        assert_eq!(operations[0].get_str("operationType").unwrap(), "retrieve");
        assert_eq!(
            operations[0]
                .get_document("constants")
                .unwrap()
                .get_str("XPath")
                .unwrap(),
            "//Demo.Item"
        );
    }
}
