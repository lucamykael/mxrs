//! Fail-closed, read-only inventory of Marketplace protocol connectors.

use std::path::Path;

use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct RegistryEntry {
    pub protocol: &'static str,
    pub name: &'static str,
    pub publisher: &'static str,
    pub category: &'static str,
    pub marketplace_id: &'static str,
    pub content_type: &'static str,
    pub appstore_guids: &'static [&'static str],
    pub source_url: &'static str,
    pub evidence_date: &'static str,
}

pub const REGISTRY: &[RegistryEntry] = &[
    RegistryEntry {
        protocol: "mqtt",
        name: "MQTT",
        publisher: "Mendix",
        category: "Messaging",
        marketplace_id: "119508",
        content_type: "Service",
        appstore_guids: &[],
        source_url: "https://marketplace.mendix.com/link/component/119508",
        evidence_date: "2026-08-04",
    },
    RegistryEntry {
        protocol: "opc_ua",
        name: "OPC-UA Connector",
        publisher: "Mendix",
        category: "Industrial",
        marketplace_id: "230843",
        content_type: "Module",
        appstore_guids: &[],
        source_url: "https://marketplace.mendix.com/link/component/230843",
        evidence_date: "2026-08-04",
    },
    RegistryEntry {
        protocol: "opc_ua",
        name: "OPC UA Client Connector",
        publisher: "Mendix",
        category: "Industrial",
        marketplace_id: "117391",
        content_type: "Service",
        appstore_guids: &[],
        source_url: "https://marketplace.mendix.com/link/component/117391",
        evidence_date: "2026-08-04",
    },
    RegistryEntry {
        protocol: "kafka",
        name: "Kafka",
        publisher: "Mendix",
        category: "Messaging",
        marketplace_id: "105878",
        content_type: "Module",
        appstore_guids: &[],
        source_url: "https://marketplace.mendix.com/link/component/105878",
        evidence_date: "2026-08-04",
    },
    RegistryEntry {
        protocol: "websocket",
        name: "WebsocketClient",
        publisher: "Mendix",
        category: "Communication",
        marketplace_id: "235426",
        content_type: "Module",
        appstore_guids: &[],
        source_url: "https://marketplace.mendix.com/link/component/235426",
        evidence_date: "2026-08-04",
    },
    RegistryEntry {
        protocol: "amqp",
        name: "eMagiz Mendix Connector (Legacy)",
        publisher: "eMagiz",
        category: "Communication",
        marketplace_id: "118800",
        content_type: "Service",
        appstore_guids: &[],
        source_url: "https://marketplace.mendix.com/link/component/118800",
        evidence_date: "2026-08-04",
    },
];

#[derive(Debug, Serialize)]
pub struct ConnectorMetadata {
    pub name: &'static str,
    pub marketplace_id: &'static str,
    pub source_url: &'static str,
}

#[derive(Debug, Serialize)]
pub struct Connector {
    pub module_name: String,
    pub protocol: &'static str,
    pub appstore_guid: String,
    pub appstore_version: String,
    pub entities: Vec<String>,
    pub microflows: Vec<String>,
    pub protected: bool,
    pub metadata: ConnectorMetadata,
}

#[derive(Debug, Serialize)]
pub struct Audit {
    pub connectors: Vec<Connector>,
    pub unknown_marketplace_modules: Vec<String>,
    pub project: String,
}

pub fn audit(path: impl AsRef<Path>) -> mxrs_model::Result<Audit> {
    audit_with_registry(path.as_ref(), REGISTRY)
}

fn audit_with_registry(path: &Path, registry: &[RegistryEntry]) -> mxrs_model::Result<Audit> {
    let project = mxrs_model::Project::open(path, true)?;
    let mut connectors = Vec::new();
    let mut unknown = Vec::new();
    for module in project
        .modules()?
        .into_iter()
        .filter(|module| module.from_app_store)
    {
        let guid = module.app_store_guid.clone().unwrap_or_default();
        let Some(entry) = registry
            .iter()
            .find(|entry| !guid.is_empty() && entry.appstore_guids.contains(&guid.as_str()))
            .copied()
        else {
            unknown.push(module.name.unwrap_or_default());
            continue;
        };
        let protected = module.export_level == "Hidden";
        connectors.push(Connector {
            module_name: module.name.clone().unwrap_or_default(),
            protocol: entry.protocol,
            appstore_guid: guid,
            appstore_version: module.app_store_version.clone().unwrap_or_default(),
            entities: if protected {
                vec![]
            } else {
                module
                    .entities()
                    .iter()
                    .filter_map(|entity| entity.name.clone())
                    .collect()
            },
            microflows: if protected {
                vec![]
            } else {
                module
                    .microflows
                    .into_iter()
                    .filter_map(|flow| flow.name)
                    .collect()
            },
            protected,
            metadata: ConnectorMetadata {
                name: entry.name,
                marketplace_id: entry.marketplace_id,
                source_url: entry.source_url,
            },
        });
    }
    for connector in &mut connectors {
        connector.entities.sort();
        connector.microflows.sort();
    }
    unknown.sort();
    Ok(Audit {
        connectors,
        unknown_marketplace_modules: unknown,
        project: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidenced_entries_sort_public_members_and_hide_protected_members() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Project.mpr");
        let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
        builder.module("Connector", |module| {
            module.entity("Zulu", |_| {});
            module.entity("Alpha", |_| {});
            module.microflow("Zulu", |_| {});
            module.microflow("Alpha", |_| {});
        });
        mxrs_writer::write_project(&path, &builder.build()).unwrap();
        let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
        let unit = mpr.units_by_containment("Modules").unwrap().remove(0);
        let mut doc = mpr.parse_contents(&unit).unwrap();
        doc.insert("FromAppStore", true);
        doc.insert("AppStoreGuid", "test-only-evidence");
        doc.insert("AppStoreVersion", "1.2.3");
        doc.insert("ExportLevel", "Source");
        mpr.update_unit(&unit.unit_id, doc.clone()).unwrap();
        drop(mpr);
        // Synthetic evidence is confined to this private test registry. It
        // must never change recognition by the shipped CLI.
        let registry = [RegistryEntry {
            appstore_guids: &["test-only-evidence"],
            ..REGISTRY[0]
        }];
        assert!(audit(&path).unwrap().connectors.is_empty());
        let result = audit_with_registry(&path, &registry).unwrap();
        assert_eq!(result.connectors[0].entities, ["Alpha", "Zulu"]);
        assert_eq!(result.connectors[0].microflows, ["Alpha", "Zulu"]);
        let json = serde_json::to_value(&result.connectors[0]).unwrap();
        assert_eq!(
            json["metadata"],
            serde_json::json!({"name": "MQTT", "marketplace_id": "119508", "source_url": "https://marketplace.mendix.com/link/component/119508"})
        );
        assert_eq!(json["appstore_version"], "1.2.3");
        doc.insert("ExportLevel", "Hidden");
        let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
        mpr.update_unit(&unit.unit_id, doc).unwrap();
        drop(mpr);
        let result = audit_with_registry(&path, &registry).unwrap();
        assert!(result.connectors[0].protected);
        assert!(result.connectors[0].entities.is_empty());
        assert!(result.connectors[0].microflows.is_empty());
    }
}
