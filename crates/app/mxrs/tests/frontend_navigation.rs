//! The frontend declares the navigation; a Rust page adds its item to the
//! profile it names there.

use mxrs::prelude::*;

#[navigation_item(profile = "Responsive", caption = "Invoices")]
pub fn invoices_navigation(item: &mut NavigationItemBuilder) {
    item.page("Sales.Invoices");
}

#[mxrs::application(version = "11.12.1")]
pub struct Application;

fn frontend(navigation: &str) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("src/navigation");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("index.ts"), navigation).unwrap();
    directory
}

#[test]
fn a_rust_page_joins_the_profile_the_frontend_declares() {
    let directory = frontend(
        "export default {\n  profiles: [\n    { name: \"Responsive\", homePage: \"Main.Home\", items: [{ caption: \"Orders\", page: \"Sales.Orders\" }] },\n  ],\n};\n",
    );
    let project = Application::build_with_frontend(directory.path()).unwrap();
    let navigation = project.navigation.unwrap();
    let [responsive] = navigation.profiles.as_slice() else {
        panic!("{navigation:?}");
    };
    assert_eq!(responsive.home_page.as_deref(), Some("Main.Home"));
    let captions: Vec<_> = responsive
        .items
        .iter()
        .map(|item| item.caption["en_US"].as_str())
        .collect();
    assert_eq!(captions, ["Orders", "Invoices"]);
}

#[test]
fn without_a_frontend_navigation_the_items_make_their_profile() {
    let directory = tempfile::tempdir().unwrap();
    let project = Application::build_with_frontend(directory.path()).unwrap();
    let navigation = project.navigation.unwrap();
    assert_eq!(navigation.profiles.len(), 1);
    assert_eq!(navigation.profiles[0].items.len(), 1);
    assert_eq!(
        Application::build().navigation,
        Some(navigation),
        "a frontend without a navigation changes nothing"
    );
}
