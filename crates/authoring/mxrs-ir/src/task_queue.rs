use crate::ExportLevel;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TaskQueueScope {
    #[default]
    PerNode,
    ClusterWide,
}

/// The legacy integer and modern expression configurations are mutually
/// exclusive. Only the modern configuration supports cluster-wide limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskQueueConfig {
    Fixed {
        parallelism: u32,
    },
    Dynamic {
        parallelism_expression: String,
        scope: TaskQueueScope,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskQueueDecl {
    pub name: String,
    pub config: TaskQueueConfig,
    pub documentation: String,
    pub excluded: bool,
    pub export_level: ExportLevel,
}

impl TaskQueueDecl {
    pub fn new(name: impl Into<String>, config: TaskQueueConfig) -> Self {
        Self {
            name: name.into(),
            config,
            documentation: String::new(),
            excluded: false,
            export_level: ExportLevel::Hidden,
        }
    }
}
