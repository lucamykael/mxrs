/// An entry point that builds part of the model by hand: the shape the
/// importer and `mxrs new` generated before declarations registered
/// themselves, and still the way to compose anything a macro does not cover.
mod composition {
    pub fn build() -> mxrs::ProjectDecl {
        let mut composed = mxrs::ProjectBuilder::new("placeholder");
        composed.module("Composed", |_module| {});
        composed.build()
    }
}

#[mxrs::application(version = "11.12.1")]
struct Application;

#[mxrs::application(version = "11.12.1", project = crate::composition::build)]
struct ComposedApplication;

#[test]
fn an_application_with_no_declarations_is_an_empty_model_of_its_version() {
    let declaration = Application::build();
    assert_eq!(declaration.mendix_version, "11.12.1");
    assert!(declaration.modules.is_empty());
}

#[test]
fn an_explicit_entry_point_contributes_what_it_builds() {
    let declaration = ComposedApplication::build();
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
