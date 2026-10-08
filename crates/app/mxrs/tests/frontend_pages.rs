//! The frontend's TSX declares pages: each file of `src/pages/<module>/` is
//! the document the model stores, read with the elements
//! `src/mxrs/elements.ts` declares.

use mxrs::prelude::*;

#[mxrs::application(version = "11.12.1")]
pub struct Application;

fn frontend(files: &[(&str, &str)]) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    for (name, source) in files {
        let path = directory.path().join("src").join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
    directory
}

const ELEMENTS: &str = r#"import { children, element } from "@/mxrs/forms";

export const Label = element("Forms$Label", {
  caption: "",
  name: "",
});

export const Page = element("Forms$Page", {
  name: "",
  title: "",
  widgets: children(2),
});
"#;

const ORDERS: &str = r#"import { Label, Page } from "@/mxrs/elements";
import { page } from "@/mxrs/forms";

export default page(
  "Sales",
  <Page name="Orders" title="All orders">
    <Label name="hint" caption="Newest first" />
  </Page>,
);
"#;

#[test]
fn a_page_the_frontend_declares_joins_its_module_as_its_document() {
    let directory = frontend(&[
        ("mxrs/elements.ts", ELEMENTS),
        ("pages/sales/Orders.tsx", ORDERS),
        // What is not in a module's folder is the application's own code.
        (
            "pages/NotFound.tsx",
            "export function NotFound() { return null; }\n",
        ),
    ]);
    let project = Application::build_with_frontend(directory.path()).unwrap();
    let sales = project
        .modules
        .iter()
        .find(|module| module.name == "Sales")
        .unwrap();
    let [orders] = sales.forms.as_slice() else {
        panic!("{:?}", sales.forms);
    };
    assert_eq!(orders.kind(), "Forms$Page");
    assert_eq!(orders.name(), "Orders");
    assert_eq!(
        orders.document,
        NativeDocument::new("Forms$Page")
            .with("Name", "Orders")
            .with("Title", "All orders")
            .with(
                "Widgets",
                NativeValue::List(
                    2,
                    vec![
                        NativeDocument::new("Forms$Label")
                            .with("Caption", "Newest first")
                            .with("Name", "hint")
                            .into()
                    ]
                )
            )
    );
}

#[test]
fn a_page_is_read_with_the_elements_the_frontend_declares() {
    let without = frontend(&[("pages/sales/Orders.tsx", ORDERS)]);
    let error = Application::build_with_frontend(without.path())
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("elements.ts") && error.contains("it is missing"),
        "{error}"
    );
    // A page is in the folder of what it is.
    let misplaced = frontend(&[
        ("mxrs/elements.ts", ELEMENTS),
        ("components/snippets/sales/Orders.tsx", ORDERS),
    ]);
    let error = Application::build_with_frontend(misplaced.path())
        .unwrap_err()
        .to_string();
    assert!(error.contains("whose place is src/pages"), "{error}");
    // A folder inside its module's folder is that module's folder.
    let nested = frontend(&[
        ("mxrs/elements.ts", ELEMENTS),
        ("pages/sales/Admin/Lists/Orders.tsx", ORDERS),
    ]);
    let project = Application::build_with_frontend(nested.path()).unwrap();
    let sales = project
        .modules
        .iter()
        .find(|module| module.name == "Sales")
        .unwrap();
    assert_eq!(sales.folders["Orders"], "Admin/Lists");
    // A form is of the module whose folder it is in.
    let elsewhere = frontend(&[
        ("mxrs/elements.ts", ELEMENTS),
        ("pages/billing/Orders.tsx", ORDERS),
    ]);
    let error = Application::build_with_frontend(elsewhere.path())
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("Orders.tsx") && error.contains("which is another module's"),
        "{error}"
    );
}
