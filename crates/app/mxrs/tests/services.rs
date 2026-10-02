//! A service holds the flows of one subject: an `impl` block whose methods
//! are the flows, each named by the convention from the service's subject
//! and the method — `ACT` + `AssetType` + `edit` is `ACT_AssetType_Edit`.

use mxrs::prelude::*;

#[module_roles(module = "Catalogs")]
pub enum Role {
    Admin,
}

#[entity(module = "Catalogs")]
pub struct AssetType {
    pub name: MxString,
}

pub struct AssetTypeService;

#[service(module = "Catalogs", subject = AssetType)]
impl AssetTypeService {
    /// Opens an asset type for editing.
    #[microflow(ACT, roles(Role::Admin))]
    pub fn edit(flow: &mut FlowBuilder) {
        flow.parameter_of("AssetType", DataType::object::<AssetType>(), |_| {});
        flow.call(MicroflowRef::<SUB_AssetType_CheckInUse>::new(), |call| {
            call.argument("AssetType", mx("$AssetType"));
        });
    }

    #[microflow(SUB)]
    pub fn check_in_use(flow: &mut FlowBuilder) {
        flow.parameter_of("AssetType", DataType::object::<AssetType>(), |_| {});
    }

    /// A name that does not follow the convention is stated.
    #[microflow(DS, name = "DS_List_AssetType")]
    pub fn list(_flow: &mut FlowBuilder) {}

    /// A method that is not a flow is left alone.
    pub fn helper() -> &'static str {
        "not a flow"
    }
}

/// A subject no entity declares is named as the model's names write it.
pub struct RubyCrudService;

#[service(module = "Catalogs", subject = "RubyCrud")]
impl RubyCrudService {
    #[microflow(MF)]
    pub fn create(_flow: &mut FlowBuilder) {}
}

/// A service without a subject holds what no subject gathers.
pub struct CatalogsService;

#[service(module = "Catalogs")]
impl CatalogsService {
    #[microflow(ACT)]
    pub fn seed(_flow: &mut FlowBuilder) {}
}

#[mxrs::application(version = "11.12.1")]
pub struct Application;

fn flow<'a>(project: &'a mxrs::ProjectDecl, name: &str) -> &'a mxrs::MicroflowDecl {
    project
        .modules
        .iter()
        .find(|module| module.name == "Catalogs")
        .expect("the Catalogs module is declared")
        .microflows
        .iter()
        .find(|flow| flow.name == name)
        .unwrap_or_else(|| panic!("{name} is declared"))
}

#[test]
fn a_services_methods_are_its_subjects_flows() {
    let project = Application::build();
    let edit = flow(&project, "ACT_AssetType_Edit");
    assert_eq!(edit.documentation, "Opens an asset type for editing.");
    assert_eq!(edit.allowed_roles, Some(vec!["Catalogs.Admin".to_string()]));
    assert_eq!(edit.parameters.len(), 1);
    assert_eq!(
        flow(&project, "SUB_AssetType_CheckInUse").parameters.len(),
        1
    );
    flow(&project, "DS_List_AssetType");
    flow(&project, "MF_RubyCrud_Create");
    flow(&project, "ACT_Seed");
    assert_eq!(AssetTypeService::helper(), "not a flow");
    // Each flow is the type its declaration generates, as anywhere else.
    assert_eq!(
        <ACT_AssetType_Edit as mxrs::FlowName>::flow_name(),
        "Catalogs.ACT_AssetType_Edit"
    );
    assert_eq!(
        <DS_List_AssetType as mxrs::FlowName>::flow_name(),
        "Catalogs.DS_List_AssetType"
    );
}
