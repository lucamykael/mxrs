use mxrs_expr::{
    Expr, FlowResultType, IntoExpr, ListVar, MemberAssignment, MendixReturnType, MxBool, MxList,
    MxObject, RenderExpr, TypedRenderExpr, Var,
};
use mxrs_ir::declaration::ModuleDecl;
use mxrs_ir::flow::{Activity, MicroflowCallMapping, MicroflowDecl};
use mxrs_ir::{EntityMarker, MicroflowMarker, MicroflowRef, Ref};

/// A module facet that can declare only server-side microflows.
///
/// Use it through [`crate::ProjectBuilder::microflow_module`] when keeping
/// application orchestration separate from domain and presentation source.
pub struct MicroflowModuleBuilder {
    name: String,
    flows: Vec<MicroflowDecl>,
}

impl MicroflowModuleBuilder {
    /// Starts a standalone server-flow module facet. Public so a service
    /// declaration that lives in its own file can assemble the one module it
    /// contributes without a whole `ProjectBuilder`.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            flows: Vec::new(),
        }
    }

    pub fn microflow(
        &mut self,
        name: impl Into<String>,
        configure: impl FnOnce(&mut FlowBuilder),
    ) -> &mut Self {
        let mut builder = FlowBuilder::new(name);
        configure(&mut builder);
        self.flows.push(builder.into_decl());
        self
    }

    /// Finishes the module facet; the counterpart of
    /// [`MicroflowModuleBuilder::new`] for a standalone declaration file.
    pub fn into_decl(self) -> ModuleDecl {
        ModuleDecl {
            name: self.name,
            microflows: self.flows,
            ..ModuleDecl::default()
        }
    }
}

/// A module facet that can declare only client-side nanoflows.
///
/// Its deliberately narrow API prevents a presentation file from silently
/// creating a server flow, or the reverse.
pub struct NanoflowModuleBuilder {
    name: String,
    flows: Vec<MicroflowDecl>,
}

impl NanoflowModuleBuilder {
    /// Starts a standalone client-flow module facet; see
    /// [`MicroflowModuleBuilder::new`].
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            flows: Vec::new(),
        }
    }

    pub fn nanoflow(
        &mut self,
        name: impl Into<String>,
        configure: impl FnOnce(&mut FlowBuilder),
    ) -> &mut Self {
        let mut builder = FlowBuilder::new(name);
        configure(&mut builder);
        self.flows.push(builder.into_decl());
        self
    }

    /// Finishes the module facet; the counterpart of
    /// [`NanoflowModuleBuilder::new`] for a standalone declaration file.
    pub fn into_decl(self) -> ModuleDecl {
        ModuleDecl {
            name: self.name,
            nanoflows: self.flows,
            ..ModuleDecl::default()
        }
    }
}

pub struct CallArgument {
    parameter: String,
    value: String,
    value_type: mxrs_ir::flow::FlowReturnType,
}

impl CallArgument {
    pub fn new(parameter: impl Into<String>, value: impl TypedRenderExpr) -> Self {
        Self {
            parameter: parameter.into(),
            value: value.render(),
            value_type: value.flow_return_type(),
        }
    }

    pub fn typed<T: MendixReturnType>(
        parameter: impl Into<String>,
        value: impl IntoExpr<T>,
    ) -> Self {
        Self::new(parameter, value.into_expr())
    }

    fn into_ir(self) -> MicroflowCallMapping {
        MicroflowCallMapping {
            parameter: self.parameter,
            value: self.value,
            value_type: Some(self.value_type),
        }
    }
}

pub struct FlowParameterBuilder<T: MendixReturnType> {
    declaration: mxrs_ir::FlowParameterDecl,
    marker: std::marker::PhantomData<T>,
}

impl<T: MendixReturnType> FlowParameterBuilder<T> {
    pub fn documentation(&mut self, value: impl Into<String>) -> &mut Self {
        self.declaration.documentation = value.into();
        self
    }

    pub fn required(&mut self, value: bool) -> &mut Self {
        self.declaration.required = value;
        self
    }

    pub fn default_value(&mut self, value: impl IntoExpr<T>) -> &mut Self {
        self.declaration.default_value = Some(value.into_expr().render());
        self
    }
}

pub struct FlowBuilder {
    decl: MicroflowDecl,
    accepts_parameters: bool,
}

impl FlowBuilder {
    pub(crate) fn new(name: impl Into<String>) -> Self {
        FlowBuilder {
            decl: MicroflowDecl::new(name),
            accepts_parameters: true,
        }
    }

    fn branch() -> Self {
        Self {
            decl: MicroflowDecl::new(String::new()),
            accepts_parameters: false,
        }
    }

    pub(crate) fn into_decl(self) -> MicroflowDecl {
        self.decl
    }

    pub fn documentation(&mut self, text: impl Into<String>) -> &mut Self {
        self.decl.documentation = text.into();
        self
    }

    /// Declares a scalar parameter and returns its typed `$name` expression.
    /// Object/list parameters can also use this method when an `Expr` is desired.
    ///
    /// # Panics
    /// Panics inside decision, loop or rescue builders: parameters belong to the
    /// outer flow signature, never to a branch of its body.
    pub fn parameter<T: MendixReturnType>(
        &mut self,
        name: impl Into<String>,
        configure: impl FnOnce(&mut FlowParameterBuilder<T>),
    ) -> Expr<T> {
        assert!(
            self.accepts_parameters,
            "flow parameters must be declared on the outer flow builder"
        );
        let name = name.into();
        let mut parameter = FlowParameterBuilder {
            declaration: mxrs_ir::FlowParameterDecl::new(&name, T::flow_return_type()),
            marker: std::marker::PhantomData,
        };
        configure(&mut parameter);
        self.decl.parameters.push(parameter.declaration);
        Expr::variable(name)
    }

    pub fn object_parameter<M: EntityMarker>(
        &mut self,
        name: impl Into<String>,
        _entity: Ref<M>,
        configure: impl FnOnce(&mut FlowParameterBuilder<MxObject<M>>),
    ) -> Var<M> {
        let name = name.into();
        self.parameter::<MxObject<M>>(&name, configure);
        Var::new(name)
    }

    pub fn list_parameter<M: EntityMarker>(
        &mut self,
        name: impl Into<String>,
        _entity: Ref<M>,
        configure: impl FnOnce(&mut FlowParameterBuilder<MxList<M>>),
    ) -> ListVar<M> {
        let name = name.into();
        self.parameter::<MxList<M>>(&name, configure);
        ListVar::new(name)
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

    /// Emits a call whose result is discarded. Set `result_variable` to None
    /// and `use_return` to false; use [`Self::call_microflow_result`] to capture
    /// a typed result. The writer rejects unchecked result captures.
    pub fn call_microflow<M: MicroflowMarker>(
        &mut self,
        target: MicroflowRef<M>,
        result_variable: Option<String>,
        use_return: bool,
        mappings: Vec<CallArgument>,
    ) -> &mut Self {
        self.decl.activities.push(Activity::CallMicroflow {
            name: target.qualified_name(),
            result_variable,
            result_type: None,
            use_return,
            mappings: mappings.into_iter().map(CallArgument::into_ir).collect(),
        });
        self
    }

    /// Captures a call result as a scalar expression, object, or list variable.
    /// The writer checks `T` against the authored or imported target signature
    /// before changing the model. A void flow cannot supply a result.
    pub fn call_microflow_result<T: FlowResultType>(
        &mut self,
        target: MicroflowRef<impl MicroflowMarker>,
        variable: impl Into<String>,
        mappings: Vec<CallArgument>,
    ) -> T::Variable {
        let variable = variable.into();
        self.decl.activities.push(Activity::CallMicroflow {
            name: target.qualified_name(),
            result_variable: Some(variable.clone()),
            result_type: Some(T::flow_return_type()),
            use_return: true,
            mappings: mappings.into_iter().map(CallArgument::into_ir).collect(),
        });
        T::result_variable(variable)
    }

    /// A two-branch if/else. `then`/`otherwise` receive a fresh sub-builder
    /// whose activities become the respective branch.
    pub fn decision(
        &mut self,
        condition: impl IntoExpr<MxBool>,
        then: impl FnOnce(&mut FlowBuilder),
        otherwise: impl FnOnce(&mut FlowBuilder),
    ) -> &mut Self {
        let mut true_builder = FlowBuilder::branch();
        then(&mut true_builder);
        let mut false_builder = FlowBuilder::branch();
        otherwise(&mut false_builder);
        self.decl.activities.push(Activity::Decision {
            condition: condition.into_expr().render(),
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
        let mut builder = FlowBuilder::branch();
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
        condition: impl IntoExpr<MxBool>,
        body: impl FnOnce(&mut FlowBuilder),
    ) -> &mut Self {
        let mut builder = FlowBuilder::branch();
        body(&mut builder);
        self.decl.activities.push(Activity::WhileLoop {
            condition: condition.into_expr().render(),
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
        let mut builder = FlowBuilder::branch();
        body(&mut builder);
        self.decl.rescue_activities = builder.decl.activities;
        self
    }

    pub fn return_value(&mut self, expression: impl TypedRenderExpr) -> &mut Self {
        self.decl.return_expression = Some(expression.render());
        self.decl.return_type = Some(expression.flow_return_type());
        self
    }
}
