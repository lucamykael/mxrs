//! `ProjectBuilder::module` can declare both flow families, which is right for
//! a file that owns a whole module but wrong for the generated
//! `src/application/services/` and `src/ui/nanoflows/`,
//! where a flow declared on the wrong side is a layering mistake nothing else
//! would catch. The typed facets exist to make that mistake fail to compile, so
//! what is asserted here is as much what the facets *cannot* reach as what they
//! produce.

#[test]
fn the_microflow_facet_reaches_only_the_server_family() {
    let mut project = mxrs_dsl::ProjectBuilder::new("11.12.1");
    project.microflow_module("Sales", |module| {
        module.microflow("ACT_CreateOrder", |_flow| {});
        module.microflow("ACT_CancelOrder", |_flow| {});
    });
    let declaration = project.build();

    let module = &declaration.modules[0];
    assert_eq!(module.name, "Sales");
    assert_eq!(
        module
            .microflows
            .iter()
            .map(|flow| flow.name.as_str())
            .collect::<Vec<_>>(),
        ["ACT_CreateOrder", "ACT_CancelOrder"],
    );
    assert!(module.nanoflows.is_empty());
    // Everything else a `ModuleBuilder` could have reached stays untouched, so
    // the facet cannot smuggle domain or presentation content into the
    // application layer either.
    assert!(module.entities.is_empty());
    assert!(module.pages.is_empty());
    assert!(module.layouts.is_empty());
    assert!(module.roles.is_none());
}

#[test]
fn the_nanoflow_facet_reaches_only_the_client_family() {
    let mut project = mxrs_dsl::ProjectBuilder::new("11.12.1");
    project.nanoflow_module("Sales", |module| {
        module.nanoflow("NAN_RefreshOrder", |_flow| {});
    });
    let declaration = project.build();

    let module = &declaration.modules[0];
    assert_eq!(module.name, "Sales");
    assert_eq!(
        module
            .nanoflows
            .iter()
            .map(|flow| flow.name.as_str())
            .collect::<Vec<_>>(),
        ["NAN_RefreshOrder"],
    );
    assert!(module.microflows.is_empty());
    assert!(module.entities.is_empty());
    assert!(module.pages.is_empty());
}

/// Both generated flow modules merge through `merge_module`, and each runs
/// against a project the other layers have already contributed to. Neither may
/// erase what the other declared for the same module name.
#[test]
fn facet_declarations_merge_into_one_module_without_displacing_each_other() {
    let mut domain = mxrs_dsl::ProjectBuilder::new("11.12.1");
    domain.module("Sales", |module| {
        module.entity("Order", |_entity| {});
    });
    let mut declaration = domain.build();

    let mut application = mxrs_dsl::ProjectBuilder::new("11.12.1");
    application.microflow_module("Sales", |module| {
        module.microflow("ACT_CreateOrder", |_flow| {});
    });
    for declared in application.build().modules {
        declaration.merge_module(declared);
    }

    let mut presentation = mxrs_dsl::ProjectBuilder::new("11.12.1");
    presentation.nanoflow_module("Sales", |module| {
        module.nanoflow("NAN_RefreshOrder", |_flow| {});
    });
    presentation.nanoflow_module("Support", |module| {
        module.nanoflow("NAN_Ping", |_flow| {});
    });
    for declared in presentation.build().modules {
        declaration.merge_module(declared);
    }

    // One `Sales`, not three: a second module under the same name would be
    // synchronized twice by the writer.
    assert_eq!(
        declaration
            .modules
            .iter()
            .map(|module| module.name.as_str())
            .collect::<Vec<_>>(),
        ["Sales", "Support"],
    );
    let sales = &declaration.modules[0];
    assert_eq!(sales.entities.len(), 1);
    assert_eq!(sales.microflows.len(), 1);
    assert_eq!(sales.nanoflows.len(), 1);
    // A module only the presentation layer declares still arrives whole.
    assert_eq!(declaration.modules[1].nanoflows.len(), 1);
}
