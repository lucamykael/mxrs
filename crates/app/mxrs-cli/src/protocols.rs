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
pub struct Connector {
    pub module_name: String,
    pub protocol: &'static str,
    pub appstore_guid: String,
    pub appstore_version: String,
    pub entities: Vec<String>,
    pub microflows: Vec<String>,
    pub protected: bool,
    pub metadata: RegistryEntry,
}

#[derive(Debug, Serialize)]
pub struct Audit {
    pub connectors: Vec<Connector>,
    pub unknown_marketplace_modules: Vec<String>,
    pub project: String,
}

pub fn audit(path: impl AsRef<Path>) -> mxrs_model::Result<Audit> {
    let path = path.as_ref();
    let project = mxrs_model::Project::open(path, true)?;
    let mut connectors = Vec::new();
    let mut unknown = Vec::new();
    for module in project
        .modules()?
        .into_iter()
        .filter(|module| module.from_app_store)
    {
        let guid = module.app_store_guid.clone().unwrap_or_default();
        let Some(entry) = REGISTRY
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
            metadata: entry,
        });
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
