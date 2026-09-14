mod domain {
    pub fn build() -> mxrs::ProjectDecl {
        mxrs::ProjectBuilder::new("placeholder").build()
    }
}

/// The layered shape `mxrs import` now emits: the crate root enters the model
/// through a composition layer instead of the `crate::domain::build` default.
mod application {
    pub fn build() -> mxrs::ProjectDecl {
        let mut project = crate::domain::build();
        let mut composed = mxrs::ProjectBuilder::new("placeholder");
        composed.module("Composed", |_module| {});
        project.modules.extend(composed.build().modules);
        project
    }
}

#[mxrs::application(version = "11.12.1")]
struct Application;

#[mxrs::application(version = "11.12.1", project = crate::application::build)]
struct LayeredApplication;

#[test]
fn application_macro_uses_the_domain_composition_root_and_target_version() {
    let declaration = Application::build();
    assert_eq!(declaration.mendix_version, "11.12.1");
    assert!(declaration.modules.is_empty());
}

#[test]
fn application_macro_enters_the_model_through_an_explicit_project_entry_point() {
    let declaration = LayeredApplication::build();
    assert_eq!(declaration.mendix_version, "11.12.1");
    assert_eq!(
        declaration
            .modules
            .iter()
            .map(|module| module.name.as_str())
            .collect::<Vec<_>>(),
        ["Composed"],
    );
}
