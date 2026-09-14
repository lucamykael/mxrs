use mxrs_ir::declaration::ProjectDecl;

use crate::flow::{MicroflowModuleBuilder, NanoflowModuleBuilder};
use crate::module::ModuleBuilder;
use crate::navigation::NavigationBuilder;
use crate::security::SecurityBuilder;

pub struct ProjectBuilder {
    version: String,
    modules: Vec<mxrs_ir::declaration::ModuleDecl>,
    security: Option<mxrs_ir::ProjectSecurityDecl>,
    navigation: Option<mxrs_ir::NavigationDecl>,
}

impl ProjectBuilder {
    pub fn new(mendix_version: impl Into<String>) -> Self {
        ProjectBuilder {
            version: mendix_version.into(),
            modules: vec![],
            security: None,
            navigation: None,
        }
    }

    pub fn module(
        &mut self,
        name: impl Into<String>,
        configure: impl FnOnce(&mut ModuleBuilder),
    ) -> &mut Self {
        let mut builder = ModuleBuilder::new(name);
        configure(&mut builder);
        self.modules.push(builder.into_decl());
        self
    }

    /// Declares only server-side microflows for one Mendix module.
    pub fn microflow_module(
        &mut self,
        name: impl Into<String>,
        configure: impl FnOnce(&mut MicroflowModuleBuilder),
    ) -> &mut Self {
        let mut builder = MicroflowModuleBuilder::new(name);
        configure(&mut builder);
        self.modules.push(builder.into_decl());
        self
    }

    /// Declares only client-side nanoflows for one Mendix module.
    pub fn nanoflow_module(
        &mut self,
        name: impl Into<String>,
        configure: impl FnOnce(&mut NanoflowModuleBuilder),
    ) -> &mut Self {
        let mut builder = NanoflowModuleBuilder::new(name);
        configure(&mut builder);
        self.modules.push(builder.into_decl());
        self
    }

    pub fn build(self) -> ProjectDecl {
        ProjectDecl {
            mendix_version: self.version,
            modules: self.modules,
            security: self.security,
            navigation: self.navigation,
        }
    }

    pub fn security(&mut self, configure: impl FnOnce(&mut SecurityBuilder)) -> &mut Self {
        let mut builder = SecurityBuilder::new();
        configure(&mut builder);
        self.security = Some(builder.into_decl());
        self
    }

    pub fn navigation(&mut self, configure: impl FnOnce(&mut NavigationBuilder)) -> &mut Self {
        let mut builder = NavigationBuilder::new();
        configure(&mut builder);
        self.navigation = Some(builder.into_decl());
        self
    }
}
