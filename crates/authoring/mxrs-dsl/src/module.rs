use mxrs_ir::ScheduleUnit;
use mxrs_ir::declaration::{ModuleDecl, OqlViewSourceDecl};

use crate::constant::ConstantBuilder;
use crate::entity::EntityBuilder;
use crate::enumeration::EnumerationBuilder;
use crate::flow::FlowBuilder;
use crate::menu::MenuBuilder;
use crate::page::{LayoutBuilder, PageBuilder};
use crate::regular_expression::RegularExpressionBuilder;
use crate::scheduled_event::ScheduledEventBuilder;

pub struct ModuleBuilder {
    decl: ModuleDecl,
}

pub struct OqlViewSourceBuilder {
    decl: OqlViewSourceDecl,
}

impl OqlViewSourceBuilder {
    pub fn documentation(&mut self, value: impl Into<String>) -> &mut Self {
        self.decl.documentation = value.into();
        self
    }

    pub fn excluded(&mut self, value: bool) -> &mut Self {
        self.decl.excluded = value;
        self
    }

    pub fn export_level(&mut self, value: mxrs_ir::ExportLevel) -> &mut Self {
        self.decl.export_level = value;
        self
    }
}

impl ModuleBuilder {
    /// Starts a standalone module. Public so a declaration that lives in its
    /// own file — an entity generated as `#[derive(MxEntity)]`, say — can
    /// assemble the one module it contributes without dragging in a whole
    /// [`ProjectBuilder`](crate::ProjectBuilder).
    pub fn new(name: impl Into<String>) -> Self {
        ModuleBuilder {
            decl: ModuleDecl {
                name: name.into(),
                entities: vec![],
                oql_view_sources: vec![],
                enumerations: vec![],
                constants: vec![],
                regular_expressions: vec![],
                task_queues: vec![],
                scheduled_events: vec![],
                menus: vec![],
                microflows: vec![],
                nanoflows: vec![],
                pages: vec![],
                layouts: vec![],
                forms: Vec::new(),
                roles: None,
            },
        }
    }

    /// Finishes the module. The counterpart of [`ModuleBuilder::new`] for a
    /// standalone declaration file.
    pub fn into_decl(self) -> ModuleDecl {
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

    pub fn oql_view_source(
        &mut self,
        name: impl Into<String>,
        query: impl Into<String>,
        configure: impl FnOnce(&mut OqlViewSourceBuilder),
    ) -> &mut Self {
        let mut builder = OqlViewSourceBuilder {
            decl: OqlViewSourceDecl::new(name, query),
        };
        configure(&mut builder);
        self.decl.oql_view_sources.push(builder.decl);
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

    /// Declares a project constant. Defaults to a `String` constant with an
    /// empty value, which is what `mxrs constant new` scaffolds.
    pub fn constant(
        &mut self,
        name: impl Into<String>,
        configure: impl FnOnce(&mut ConstantBuilder),
    ) -> &mut Self {
        let mut builder = ConstantBuilder::new(name);
        configure(&mut builder);
        self.decl.constants.push(builder.into_decl());
        self
    }

    pub fn task_queue(
        &mut self,
        name: impl Into<String>,
        config: mxrs_ir::TaskQueueConfig,
        configure: impl FnOnce(&mut crate::TaskQueueBuilder),
    ) -> &mut Self {
        let mut builder = crate::TaskQueueBuilder::new(name, config);
        configure(&mut builder);
        self.decl.task_queues.push(builder.into_decl());
        self
    }

    pub fn regular_expression(
        &mut self,
        name: impl Into<String>,
        expression: impl Into<String>,
        configure: impl FnOnce(&mut RegularExpressionBuilder),
    ) -> &mut Self {
        let mut builder = RegularExpressionBuilder::new(name, expression);
        configure(&mut builder);
        self.decl.regular_expressions.push(builder.into_decl());
        self
    }

    /// Declares a scheduled event running `microflow` every `unit`.
    ///
    /// `microflow` may be unqualified (same module) or `"Module.Microflow"`;
    /// the writer stores the qualified form either way.
    pub fn scheduled_event(
        &mut self,
        name: impl Into<String>,
        microflow: impl Into<String>,
        unit: ScheduleUnit,
        configure: impl FnOnce(&mut ScheduledEventBuilder),
    ) -> &mut Self {
        let mut builder = ScheduledEventBuilder::new(name, microflow, unit);
        configure(&mut builder);
        self.decl.scheduled_events.push(builder.into_decl());
        self
    }

    /// Declares a standalone menu document. This is distinct from the
    /// project navigation profiles and can be referenced by menu widgets.
    pub fn menu(
        &mut self,
        name: impl Into<String>,
        configure: impl FnOnce(&mut MenuBuilder),
    ) -> &mut Self {
        let mut builder = MenuBuilder::new(name);
        configure(&mut builder);
        self.decl.menus.push(builder.into_decl());
        self
    }

    pub fn role(&mut self, name: impl Into<String>, description: impl Into<String>) -> &mut Self {
        let mut role = mxrs_ir::ModuleRoleDecl::new(name);
        role.description = description.into();
        self.decl.roles.get_or_insert_with(Vec::new).push(role);
        self
    }

    pub fn clear_roles(&mut self) -> &mut Self {
        self.decl.roles = Some(vec![]);
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

    pub fn layout(
        &mut self,
        name: impl Into<String>,
        configure: impl FnOnce(&mut LayoutBuilder),
    ) -> &mut Self {
        let mut builder = LayoutBuilder::new(name);
        configure(&mut builder);
        self.decl.layouts.push(builder.into_decl());
        self
    }
}
