use mxrs_ir::declaration::ModuleDecl;

use crate::entity::EntityBuilder;
use crate::enumeration::EnumerationBuilder;
use crate::flow::FlowBuilder;
use crate::page::PageBuilder;

pub struct ModuleBuilder {
    decl: ModuleDecl,
}

impl ModuleBuilder {
    pub(crate) fn new(name: impl Into<String>) -> Self {
        ModuleBuilder {
            decl: ModuleDecl {
                name: name.into(),
                entities: vec![],
                enumerations: vec![],
                microflows: vec![],
                nanoflows: vec![],
                pages: vec![],
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

    /// Declares a client-side nanoflow using the same typed semantic flow
    /// builder as microflows. The writer persists it under the nanoflow
    /// document kind and a separate stable identity namespace.
    pub fn nanoflow(
        &mut self,
        name: impl Into<String>,
        configure: impl FnOnce(&mut FlowBuilder),
    ) -> &mut Self {
        let mut builder = FlowBuilder::new(name);
        configure(&mut builder);
        self.decl.nanoflows.push(builder.into_decl());
        self
    }

    pub fn enumeration(
        &mut self,
        name: impl Into<String>,
        configure: impl FnOnce(&mut EnumerationBuilder),
    ) -> &mut Self {
        let mut builder = EnumerationBuilder::new(name);
        configure(&mut builder);
        self.decl.enumerations.push(builder.into_decl());
        self
    }

    /// Declares a page — see `mxrs-dsl::page`'s crate-level doc comment for
    /// the first-slice widget vocabulary and what's deliberately deferred.
    pub fn page(
        &mut self,
        name: impl Into<String>,
        configure: impl FnOnce(&mut PageBuilder),
    ) -> &mut Self {
        let mut builder = PageBuilder::new(name);
        configure(&mut builder);
        self.decl.pages.push(builder.into_decl());
        self
    }
}
