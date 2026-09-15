use mxrs_ir::{ExportLevel, TaskQueueConfig, TaskQueueDecl};

pub struct TaskQueueBuilder {
    declaration: TaskQueueDecl,
}

impl TaskQueueBuilder {
    pub(crate) fn new(name: impl Into<String>, config: TaskQueueConfig) -> Self {
        Self {
            declaration: TaskQueueDecl::new(name, config),
        }
    }

    pub(crate) fn into_decl(self) -> TaskQueueDecl {
        self.declaration
    }

    pub fn documentation(&mut self, text: impl Into<String>) -> &mut Self {
        self.declaration.documentation = text.into();
        self
    }

    pub fn excluded(&mut self, excluded: bool) -> &mut Self {
        self.declaration.excluded = excluded;
        self
    }

    pub fn export_level(&mut self, level: ExportLevel) -> &mut Self {
        self.declaration.export_level = level;
        self
    }
}
