mod domain {
    pub fn build() -> mxrs::ProjectDecl {
        mxrs::ProjectBuilder::new("placeholder").build()
    }
}

#[mxrs::application(version = "11.12.1")]
struct Application;

#[test]
fn application_macro_uses_the_domain_composition_root_and_target_version() {
    let declaration = Application::build();
    assert_eq!(declaration.mendix_version, "11.12.1");
}
