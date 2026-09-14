//! Atomic functional-test instrumentation of a disposable MPR.

use mxrs_bson::{Bson, build_array, parse_array};
use mxrs_identity::{ArtifactKind, ProjectIdentity};
use mxrs_ir::flow::MicroflowDecl;
use mxrs_model::{DomainModel, Microflow, Module};
use mxrs_mpr::MprFile;

use crate::{Result, WriterError, flow_compiler};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstrumentationReport {
    pub module: String,
    pub runner: String,
    pub tests: usize,
}

pub fn instrument_functional_tests(
    path: impl AsRef<std::path::Path>,
    module_name: &str,
    runner: &str,
    flows: &[MicroflowDecl],
    tests: usize,
) -> Result<InstrumentationReport> {
    let mut mpr = MprFile::open(path, false)?;
    let root_id = mpr
        .root_unit()?
        .ok_or(WriterError::MissingRootUnit)?
        .unit_id;
    let identity = ProjectIdentity::from_project_root(&root_id)?;

    for unit in mpr.children_of(&root_id)? {
        let document = mpr.parse_contents(&unit)?;
        if document.get_str("$Type").ok() == Some("Projects$Module")
            && document.get_str("Name").ok() == Some(module_name)
        {
            return Err(WriterError::InstrumentationModuleExists(
                module_name.to_string(),
            ));
        }
    }

    let settings_unit = mpr
        .all_units()?
        .into_iter()
        .find_map(|unit| {
            let document = mpr.parse_contents(&unit).ok()?;
            (document.get_str("$Type").ok() == Some("Settings$ProjectSettings"))
                .then_some((unit.unit_id, document))
        })
        .ok_or(WriterError::MissingProjectSettings)?;
    let (settings_id, mut settings_document) = settings_unit;
    let raw_settings = settings_document
        .get_array("Settings")
        .map_err(|_| WriterError::MissingModelSettings)?;
    let parsed = parse_array(Some(raw_settings));
    let mut found_model = false;
    let settings = parsed
        .items
        .into_iter()
        .map(|value| match value {
            Bson::Document(mut document)
                if document.get_str("$Type").ok() == Some("Settings$ModelSettings") =>
            {
                found_model = true;
                document.insert("AfterStartupMicroflow", runner);
                Bson::Document(document)
            }
            value => value,
        })
        .collect();
    if !found_model {
        return Err(WriterError::MissingModelSettings);
    }
    settings_document.insert("Settings", build_array(settings, parsed.marker));

    let module_id = identity.artifact_id(ArtifactKind::Module, module_name);
    let module_document = Module {
        id: module_id.clone(),
        name: Some(module_name.to_string()),
        sort_index: None,
        from_app_store: false,
        app_store_guid: None,
        app_store_version: None,
        export_level: "Hidden".into(),
        domain_model: None,
        pages: vec![],
        microflows: vec![],
        nanoflows: vec![],
        rules: vec![],
        menus: vec![],
        module_roles: vec![],
        artifact_units: vec![],
    }
    .to_bson();
    let domain_id = identity.artifact_id(ArtifactKind::DomainModel, module_name);
    let domain_document = DomainModel {
        id: None,
        native_type: None,
        documentation: String::new(),
        entities: vec![],
        associations: vec![],
        cross_associations: vec![],
    }
    .to_bson(&domain_id);
    let flow_documents = flows
        .iter()
        .map(|flow| {
            let id = identity.artifact_id(
                ArtifactKind::Microflow,
                &format!("{module_name}.{}", flow.name),
            );
            let (objects, sequence_flows) = flow_compiler::build_microflow_graph(
                &flow.activities,
                &flow.rescue_activities,
                flow.return_expression.as_deref(),
            );
            let document = Microflow {
                id: Some(id.clone()),
                name: Some(flow.name.clone()),
                documentation: flow.documentation.clone(),
                return_variable_name: "ReturnValue".into(),
                allow_concurrent_execution: true,
                apply_entity_access: false,
                mark_as_used: false,
                excluded: false,
                export_level: "Hidden".into(),
                allowed_module_roles: vec![],
                parameters: vec![],
                return_type_document: flow_compiler::return_type_document(
                    flow.return_type.as_ref(),
                ),
                return_type: None,
                objects,
                flows: sequence_flows,
            }
            .to_bson();
            (id, document)
        })
        .collect::<Vec<_>>();

    mpr.transaction(move |mpr| {
        mpr.insert_unit(&root_id, "Modules", module_document, Some(&module_id))?;
        mpr.insert_unit(&module_id, "DomainModel", domain_document, Some(&domain_id))?;
        for (id, document) in flow_documents {
            mpr.insert_unit(&module_id, "Documents", document, Some(&id))?;
        }
        mpr.update_unit(&settings_id, settings_document)?;
        Ok(())
    })?;

    Ok(InstrumentationReport {
        module: module_name.to_string(),
        runner: runner.to_string(),
        tests,
    })
}
