use mxrs_ir::declaration::ProjectDecl;

use crate::module::ModuleBuilder;

pub struct ProjectBuilder {
    version: String,
    modules: Vec<mxrs_ir::declaration::ModuleDecl>,
}

impl ProjectBuilder {
    pub fn new(mendix_version: impl Into<String>) -> Self {
        ProjectBuilder { version: mendix_version.into(), modules: vec![] }
    }

    pub fn module(&mut self, name: impl Into<String>, configure: impl FnOnce(&mut ModuleBuilder)) -> &mut Self {
        let mut builder = ModuleBuilder::new(name);
        configure(&mut builder);
        self.modules.push(builder.into_decl());
        self
    }

    pub fn build(self) -> ProjectDecl {
        ProjectDecl { mendix_version: self.version, modules: self.modules }
    }
}
