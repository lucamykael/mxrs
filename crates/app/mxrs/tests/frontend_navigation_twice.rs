//! A navigation is declared once: in the frontend, or with `#[navigation]`.

use mxrs::prelude::*;

#[navigation]
pub fn navigation(navigation: &mut NavigationBuilder) {
    // Only items: still a navigation of its own.
    navigation.profile("Phone", |_| {});
}

#[mxrs::application(version = "11.12.1")]
pub struct Application;

#[test]
fn a_navigation_declared_in_rust_and_in_the_frontend_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("src/navigation");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("index.ts"),
        "export default { profiles: [{ name: \"Responsive\" }] };\n",
    )
    .unwrap();
    let error = Application::build_with_frontend(directory.path()).unwrap_err();
    assert!(
        matches!(&error, mxrs::FrontendError::Duplicate(at) if at.contains("frontend_navigation_twice.rs")),
        "{error}"
    );

    // Without one in the frontend, the Rust navigation is the project's.
    let empty = tempfile::tempdir().unwrap();
    let project = Application::build_with_frontend(empty.path()).unwrap();
    assert_eq!(project.navigation.unwrap().profiles[0].name, "Phone");
}
