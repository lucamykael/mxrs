use mxrs_expr::{Expr, ListVar, MemberAssignment, MxBool, RenderExpr, Var};
use mxrs_ir::flow::{Activity, MicroflowCallMapping, MicroflowDecl};
use mxrs_ir::{EntityMarker, Ref};

pub struct CallArgument {
    parameter: String,
    value: String,
}

impl CallArgument {
    pub fn new(parameter: impl Into<String>, value: impl RenderExpr) -> Self {
        Self {
            parameter: parameter.into(),
            value: value.render(),
        }
    }

    fn into_ir(self) -> MicroflowCallMapping {
        MicroflowCallMapping {
            parameter: self.parameter,
            value: self.value,
        }
    }
}

pub struct FlowBuilder {
    decl: MicroflowDecl,
}

impl FlowBuilder {
    pub(crate) fn new(name: impl Into<String>) -> Self {
        FlowBuilder {
            decl: MicroflowDecl::new(name),
        }
    }

    pub(crate) fn into_decl(self) -> MicroflowDecl {
        self.decl
    }

    pub fn documentation(&mut self, text: impl Into<String>) -> &mut Self {
        self.decl.documentation = text.into();
        self
    }

    pub fn create_object<M: EntityMarker>(
        &mut self,
        variable: impl Into<String>,
        entity: Ref<M>,
        members: Vec<MemberAssignment<M>>,
        commit: bool,
    ) -> Var<M> {
        let variable = variable.into();
        self.decl.activities.push(Activity::CreateObject {
            variable: variable.clone(),
            entity: entity.qualified_name(),
            members: members
                .into_iter()
                .map(MemberAssignment::into_member)
                .collect(),
            commit,
        });
        Var::new(variable)
    }

    pub fn change_object<M: EntityMarker>(
        &mut self,
        variable: &Var<M>,
        members: Vec<MemberAssignment<M>>,
        commit: bool,
    ) -> &mut Self {
        self.decl.activities.push(Activity::ChangeObject {
            variable: variable.name().to_string(),
            entity: M::qualified_name(),
            members: members
                .into_iter()
                .map(MemberAssignment::into_member)
                .collect(),
            commit,
        });
        self
    }

    pub fn commit<M: EntityMarker>(&mut self, variable: &Var<M>) -> &mut Self {
        self.decl.activities.push(Activity::Commit {
            variable: variable.name().to_string(),
        });
        self
    }

    pub fn delete_object<M: EntityMarker>(&mut self, variable: &Var<M>) -> &mut Self {
        self.decl.activities.push(Activity::DeleteObject {
            variable: variable.name().to_string(),
        });
        self
    }

    pub fn create_list<M: EntityMarker>(
        &mut self,
        variable: impl Into<String>,
        entity: Ref<M>,
    ) -> ListVar<M> {
        let variable = variable.into();
        self.decl.activities.push(Activity::CreateList {
            variable: variable.clone(),
            entity: entity.qualified_name(),
        });
        ListVar::new(variable)
    }

    pub fn call_microflow(
        &mut self,
        name: impl Into<String>,
        result_variable: Option<String>,
        use_return: bool,
        mappings: Vec<CallArgument>,
    ) -> &mut Self {
        self.decl.activities.push(Activity::CallMicroflow {
            name: name.into(),
            result_variable,
            use_return,
            mappings: mappings.into_iter().map(CallArgument::into_ir).collect(),
        });
        self
    }

    /// A two-branch if/else. `then`/`otherwise` receive a fresh sub-builder
    /// whose activities become the respective branch.
    pub fn decision(
        &mut self,
        condition: Expr<MxBool>,
        then: impl FnOnce(&mut FlowBuilder),
        otherwise: impl FnOnce(&mut FlowBuilder),
    ) -> &mut Self {
        let mut true_builder = FlowBuilder::new(String::new());
        then(&mut true_builder);
        let mut false_builder = FlowBuilder::new(String::new());
        otherwise(&mut false_builder);
        self.decl.activities.push(Activity::Decision {
            condition: condition.render(),
            true_branch: true_builder.decl.activities,
            false_branch: false_builder.decl.activities,
        });
        self
    }

    pub fn loop_over<M: EntityMarker>(
        &mut self,
        list: &ListVar<M>,
        iterator_name: impl Into<String>,
        body: impl FnOnce(&mut FlowBuilder, Var<M>),
    ) -> &mut Self {
        let iterator_name = iterator_name.into();
        let mut builder = FlowBuilder::new(String::new());
        body(&mut builder, Var::new(iterator_name.clone()));
        self.decl.activities.push(Activity::LoopOver {
            list_variable: list.name().to_string(),
            iterator: iterator_name,
            activities: builder.decl.activities,
        });
        self
    }

    pub fn while_loop(
        &mut self,
        condition: Expr<MxBool>,
        body: impl FnOnce(&mut FlowBuilder),
    ) -> &mut Self {
        let mut builder = FlowBuilder::new(String::new());
        body(&mut builder);
        self.decl.activities.push(Activity::WhileLoop {
            condition: condition.render(),
            activities: builder.decl.activities,
        });
        self
    }

    pub fn break_loop(&mut self) -> &mut Self {
        self.decl.activities.push(Activity::BreakLoop);
        self
    }

    pub fn continue_loop(&mut self) -> &mut Self {
        self.decl.activities.push(Activity::ContinueLoop);
        self
    }

    pub fn rescue_all(&mut self, body: impl FnOnce(&mut FlowBuilder)) -> &mut Self {
        let mut builder = FlowBuilder::new(String::new());
        body(&mut builder);
        self.decl.rescue_activities = builder.decl.activities;
        self
    }

    pub fn return_value(&mut self, expression: impl RenderExpr) -> &mut Self {
        self.decl.return_expression = Some(expression.render());
        self
    }
}
