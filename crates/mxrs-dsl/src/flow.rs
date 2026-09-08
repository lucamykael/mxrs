use mxrs_ir::flow::{Activity, MicroflowCallMapping, MicroflowDecl};
use mxrs_ir::Member;

pub struct FlowBuilder {
    decl: MicroflowDecl,
}

impl FlowBuilder {
    pub(crate) fn new(name: impl Into<String>) -> Self {
        FlowBuilder { decl: MicroflowDecl::new(name) }
    }

    pub(crate) fn into_decl(self) -> MicroflowDecl {
        self.decl
    }

    pub fn documentation(&mut self, text: impl Into<String>) -> &mut Self {
        self.decl.documentation = text.into();
        self
    }

    pub fn create_object(
        &mut self,
        variable: impl Into<String>,
        entity: impl Into<String>,
        members: Vec<Member>,
        commit: bool,
    ) -> &mut Self {
        self.decl.activities.push(Activity::CreateObject { variable: variable.into(), entity: entity.into(), members, commit });
        self
    }

    pub fn change_object(
        &mut self,
        variable: impl Into<String>,
        entity: impl Into<String>,
        members: Vec<Member>,
        commit: bool,
    ) -> &mut Self {
        self.decl.activities.push(Activity::ChangeObject { variable: variable.into(), entity: entity.into(), members, commit });
        self
    }

    pub fn commit(&mut self, variable: impl Into<String>) -> &mut Self {
        self.decl.activities.push(Activity::Commit { variable: variable.into() });
        self
    }

    pub fn delete_object(&mut self, variable: impl Into<String>) -> &mut Self {
        self.decl.activities.push(Activity::DeleteObject { variable: variable.into() });
        self
    }

    pub fn call_microflow(
        &mut self,
        name: impl Into<String>,
        result_variable: Option<String>,
        use_return: bool,
        mappings: Vec<MicroflowCallMapping>,
    ) -> &mut Self {
        self.decl.activities.push(Activity::CallMicroflow { name: name.into(), result_variable, use_return, mappings });
        self
    }

    /// A two-branch if/else. `then`/`otherwise` receive a fresh sub-builder
    /// whose activities become the respective branch.
    pub fn decision(
        &mut self,
        condition: impl Into<String>,
        then: impl FnOnce(&mut FlowBuilder),
        otherwise: impl FnOnce(&mut FlowBuilder),
    ) -> &mut Self {
        let mut true_builder = FlowBuilder::new(String::new());
        then(&mut true_builder);
        let mut false_builder = FlowBuilder::new(String::new());
        otherwise(&mut false_builder);
        self.decl.activities.push(Activity::Decision {
            condition: condition.into(),
            true_branch: true_builder.decl.activities,
            false_branch: false_builder.decl.activities,
        });
        self
    }

    pub fn return_value(&mut self, expression: impl Into<String>) -> &mut Self {
        self.decl.return_expression = Some(expression.into());
        self
    }
}
