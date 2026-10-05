//! A nanoflow Rust names as the frontend's is one the frontend declares,
//! and it is declared once: there, or with `#[nanoflow]`.

use mxrs::prelude::*;

#[mxrs::application(version = "11.12.1")]
pub struct Application;

mxrs::frontend_flows! {
    module = "Sales";
    nanoflow ACT_Order_Open;
}

#[nanoflow(ACT, module = "Sales")]
pub fn order_close(_flow: &mut FlowBuilder) {}

fn frontend(service: &str) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("src/services/sales");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("orderService.ts"), service).unwrap();
    directory
}

fn service(nanoflows: &[&str]) -> String {
    let mut source = String::from("export const OrderService = nanoflowService(\"Sales\", {\n");
    for (index, nanoflow) in nanoflows.iter().enumerate() {
        source.push_str(&format!(
            "  /** @nanoflow {nanoflow} */\n  async f{index}(): Promise<void> {{}},\n"
        ));
    }
    source.push_str("});\n");
    source
}

#[test]
fn a_name_rust_gives_a_frontend_nanoflow_needs_its_declaration() {
    let declared = frontend(&service(&["ACT_Order_Open"]));
    let project = Application::build_with_frontend(declared.path()).unwrap();
    let sales = project
        .modules
        .iter()
        .find(|module| module.name == "Sales")
        .unwrap();
    let mut names: Vec<&str> = sales
        .nanoflows
        .iter()
        .map(|nanoflow| nanoflow.name.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["ACT_OrderClose", "ACT_Order_Open"]);

    // Renamed or removed in the frontend, the name Rust still gives it is
    // refused where Rust gives it.
    let renamed = frontend(&service(&["ACT_Order_Show"]));
    let error = Application::build_with_frontend(renamed.path())
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("frontend_nanoflows.rs:9:")
            && error.contains("names the nanoflow Sales.ACT_Order_Open"),
        "{error}"
    );
}

#[test]
fn a_nanoflow_declared_in_rust_and_in_the_frontend_is_refused_where_the_frontend_declares_it() {
    let twice = frontend(&service(&["ACT_Order_Open", "ACT_OrderClose"]));
    let error = Application::build_with_frontend(twice.path())
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("orderService.ts:5:") && error.contains("in Rust with #[nanoflow]"),
        "{error}"
    );
}
