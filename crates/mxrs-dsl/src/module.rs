use mxrs_ir::declaration::ModuleDecl;

use crate::entity::EntityBuilder;
use crate::flow::FlowBuilder;

pub struct ModuleBuilder {
    decl: ModuleDecl,
}

impl ModuleBuilder {
    pub(crate) fn new(name: impl Into<String>) -> Self {
        ModuleBuilder {
            decl: ModuleDecl {
                name: name.into(),
                entities: vec![],
                microflows: vec![],
            },
        }
    }

    pub(crate) fn into_decl(self) -> ModuleDecl {
        self.decl
    }

    pub fn entity(
        &mut self,
        name: impl Into<String>,
        configure: impl FnOnce(&mut EntityBuilder),
    ) -> &mut Self {
        let mut builder = EntityBuilder::new(name);
        configure(&mut builder);
        self.decl.entities.push(builder.into_decl());
        self
    }

    pub fn microflow(
        &mut self,
        name: impl Into<String>,
        configure: impl FnOnce(&mut FlowBuilder),
    ) -> &mut Self {
        let mut builder = FlowBuilder::new(name);
        configure(&mut builder);
        self.decl.microflows.push(builder.into_decl());
        self
    }
}
