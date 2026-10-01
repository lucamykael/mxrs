//! The application-facing authoring surface: every declaration here registers
//! itself, and `#[mxrs::application]` assembles the model from this crate's
//! registrations alone.

use mxrs::prelude::*;

/// Where an order stands.
#[enumeration(module = "Sales")]
pub enum OrderStatus {
    Open,
    #[mxrs(caption = "Closed order")]
    Closed,
    #[mxrs(name = "on_hold", captions(en_US = "On hold", nl_NL = "In de wacht"))]
    OnHold,
}

/// Someone who orders.
#[entity(module = "Sales")]
pub struct Customer {
    #[mxrs(length = 120, required)]
    pub name: MxString,
    #[mxrs(unlimited)]
    pub notes: MxString,
}

/// A customer order.
///
/// Totals are in the shop currency.
#[entity(module = "Sales")]
#[mxrs(
    index(number),
    index(desc(total), system(ChangedDate), include_offline)
)]
#[mxrs(
    before_commit = VAL_ValidateOrder,
    after_delete("Sales.SUB_Audit", pass_event_object = false)
)]
pub struct Order {
    /// Printed on the invoice.
    #[mxrs(required, unique)]
    pub number: MxString,
    pub total: MxDecimal,
    #[mxrs(default = true)]
    pub paid: MxBool,
    pub shipped: MxBool,
    pub placed: MxDateTime,
    #[mxrs(localize_date = false)]
    pub exported: MxDateTime,
    pub lines: MxInteger,
    #[mxrs(no_default)]
    pub discount: MxDecimal,
    pub status: OrderStatus,
    /// Who placed it.
    pub customer: Reference<Customer>,
    #[mxrs(association = "Order_Watchers", owner = "Both", storage = "Table")]
    pub watchers: ReferenceSet<Customer>,
}

#[dto(module = "Sales", name = "Order_Summary")]
pub struct OrderSummary {
    pub count: MxInteger,
}

#[view(module = "Sales", source = "Sales.OrderTotals")]
pub struct OrderTotal {
    pub total: MxDecimal,
}

/// Declared for its type alone: an installed module owns it.
#[entity(module = "Catalog", imported)]
pub struct Product {
    pub code: MxString,
}

#[enumeration(module = "Catalog", imported)]
pub enum Availability {
    InStock,
}

#[microflow(VAL, module = "Sales")]
pub fn validate_order(flow: &mut FlowBuilder) {
    let order = flow.object_parameter("Order", Ref::<Order>::new(), |parameter| {
        parameter.required(true);
    });
    flow.return_value(order.get(Order::number()).is_empty().negate());
}

/// Creates an order.
#[microflow(ACT, module = "Sales")]
pub fn create_order(flow: &mut FlowBuilder) {
    let number = flow.parameter::<MxString>("Number", |parameter| {
        parameter.required(true);
    });
    let customer = flow.object_parameter("Customer", Ref::<Customer>::new(), |_| {});
    let order = flow.create_object(
        "Order",
        Ref::<Order>::new(),
        vec![
            Order::number().set(number),
            Order::paid().set(false),
            Order::customer().set(&customer),
        ],
        true,
    );
    flow.call_microflow(
        MicroflowRef::<VAL_ValidateOrder>::new(),
        None,
        false,
        vec![],
    );
    flow.return_value(order);
}

#[microflow(module = "Sales", name = "Legacy_Cleanup")]
pub fn cleanup(_flow: &mut FlowBuilder) {}

#[nanoflow(NAN, module = "Sales")]
pub fn open_order(_flow: &mut FlowBuilder) {}

mxrs::imported! {
    module = "Sales";
    microflow SUB_Audit;
    microflow return_ = "return";
    nanoflow NAN_Refresh;
}

mxrs::register!(Document, |project| {
    let mut module = ModuleBuilder::new("Sales");
    module.constant("Region", |constant| {
        constant.value("EU");
    });
    project.merge_module(module.into_decl());
});

/// The shop's home region.
#[constant(module = "Sales")]
pub fn home_region(constant: &mut ConstantBuilder) {
    constant.value("EU");
}

#[layout(module = "Sales")]
pub fn application_layout(layout: &mut LayoutBuilder) {
    layout.placeholder("Main");
}

/// Lists the open orders.
#[page(module = "Sales", name = "Order_Overview")]
pub fn order_overview(page: &mut PageBuilder) {
    page.title("Orders");
    page.layout("Sales.ApplicationLayout", "Main");
    page.data_view_from_microflow(MicroflowRef::<ACT_CreateOrder>::new(), |view| {
        view.text_box(Order::number());
        view.check_box(Order::paid());
    });
}

#[module_roles(module = "Sales")]
pub enum Role {
    /// Runs the shop.
    Administrator,
    #[mxrs(name = "Sales_Clerk")]
    Clerk,
}

#[declaration(module = "Sales")]
pub fn order_number_format(module: &mut ModuleBuilder) {
    module.regular_expression("OrderNumberFormat", "^A-[0-9]{4}$", |_| {});
}

#[security]
pub fn security(security: &mut SecurityBuilder) {
    security.level(SecurityLevel::CheckEverything);
    security.clear_roles();
    security.role("Manager", |role| {
        role.module_role(Role::Administrator.qualified_name());
    });
}

#[demo_user(name = "demo_manager")]
pub fn manager(user: &mut DemoUserBuilder) {
    user.role("Manager");
    user.password_from_env("MXRS_DEMO_MANAGER_PASSWORD");
}

#[navigation]
pub fn navigation(navigation: &mut NavigationBuilder) {
    navigation.profile("Responsive", |profile| {
        profile.home_page("Sales.Order_Overview");
    });
}

#[navigation_item(profile = "Responsive", caption = "Orders")]
pub fn orders_item(item: &mut NavigationItemBuilder) {
    item.page("Sales.Order_Overview");
}

#[mxrs::application(version = "11.12.1")]
pub struct Application;

fn sales() -> mxrs::ModuleDecl {
    Application::build()
        .modules
        .into_iter()
        .find(|module| module.name == "Sales")
        .expect("the Sales module is declared")
}

#[test]
fn the_application_is_what_its_crate_registered() {
    let project = Application::build();
    assert_eq!(project.mendix_version, "11.12.1");
    // `imported` declarations name a type without contributing a model.
    assert_eq!(
        project
            .modules
            .iter()
            .map(|module| module.name.as_str())
            .collect::<Vec<_>>(),
        ["Sales"],
    );
    // Assembly follows the source, not link order, so it is repeatable.
    let names = |project: &mxrs::ProjectDecl| {
        project.modules[0]
            .entities
            .iter()
            .map(|entity| entity.name.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&project), names(&Application::build()));
    assert_eq!(
        names(&project),
        ["Customer", "Order", "Order_Summary", "OrderTotal"]
    );
}

#[test]
fn an_entity_states_its_whole_declaration() {
    let sales = sales();
    let order = sales
        .entities
        .iter()
        .find(|entity| entity.name == "Order")
        .unwrap();
    assert_eq!(
        order.documentation,
        "A customer order.\n\nTotals are in the shop currency."
    );
    assert!(order.persistable);
    assert_eq!(order.image, Some(mxrs::EntityImageDecl::None));
    assert_eq!(order.source, Some(mxrs::EntitySourceDecl::Stored));

    let attribute = |name: &str| {
        order
            .attributes
            .iter()
            .find(|attribute| attribute.name == name)
            .unwrap()
    };
    let number = attribute("Number");
    assert_eq!(number.documentation, "Printed on the invoice.");
    assert_eq!(number.length, Some(200));
    assert!(number.required && number.unique);
    assert_eq!(number.default_value, None);
    // Unstated options are the platform's defaults, not "whatever was there".
    assert_eq!(attribute("Total").default_value.as_deref(), Some("0"));
    assert_eq!(attribute("Paid").default_value.as_deref(), Some("true"));
    assert_eq!(attribute("Shipped").default_value.as_deref(), Some("false"));
    assert_eq!(attribute("Lines").default_value.as_deref(), Some("0"));
    assert_eq!(attribute("Discount").default_value, None);
    assert_eq!(attribute("Placed").localize_date, Some(true));
    assert_eq!(attribute("Exported").localize_date, Some(false));
    assert_eq!(
        attribute("Status").enumeration.as_deref(),
        Some("Sales.OrderStatus")
    );

    let customer = &order.associations[0];
    assert_eq!(customer.name, "Order_Customer");
    assert_eq!(customer.target, "Sales.Customer");
    assert_eq!(customer.documentation, "Who placed it.");
    let watchers = &order.associations[1];
    assert_eq!(watchers.name, "Order_Watchers");
    assert_eq!(
        watchers.association_type,
        mxrs::AssociationType::ReferenceSet
    );
    assert_eq!(watchers.owner, mxrs::AssociationOwner::Both);
    assert_eq!(watchers.storage, mxrs::AssociationStorage::Table);

    let indexes = order.indexes.as_ref().unwrap();
    assert_eq!(indexes.len(), 2);
    assert_eq!(
        indexes[0].members,
        [mxrs::IndexMemberDecl::Attribute {
            name: "Number".to_string(),
            ascending: true,
        }]
    );
    assert_eq!(
        indexes[1].members,
        [
            mxrs::IndexMemberDecl::Attribute {
                name: "Total".to_string(),
                ascending: false,
            },
            mxrs::IndexMemberDecl::System {
                member: SystemMember::ChangedDate,
                ascending: true,
            },
        ]
    );
    assert!(indexes[1].include_offline && !indexes[0].include_offline);

    let lifecycle = order.lifecycle.as_ref().unwrap();
    assert_eq!(lifecycle[0].event, LifecycleEvent::BeforeCommit);
    assert_eq!(lifecycle[0].handler, "Sales.VAL_ValidateOrder");
    assert!(lifecycle[0].pass_event_object && lifecycle[0].raise_error_on_false);
    assert_eq!(lifecycle[1].event, LifecycleEvent::AfterDelete);
    assert_eq!(lifecycle[1].handler, "Sales.SUB_Audit");
    assert!(!lifecycle[1].pass_event_object && !lifecycle[1].raise_error_on_false);
}

#[test]
fn an_entity_without_indexes_or_handlers_has_none() {
    let sales = sales();
    let customer = sales
        .entities
        .iter()
        .find(|entity| entity.name == "Customer")
        .unwrap();
    assert_eq!(customer.documentation, "Someone who orders.");
    assert_eq!(customer.indexes, Some(vec![]));
    assert_eq!(customer.lifecycle, Some(vec![]));
    assert_eq!(customer.attributes[0].length, Some(120));
    assert_eq!(customer.attributes[1].length, Some(0));
}

#[test]
fn dtos_and_views_are_not_stored() {
    let sales = sales();
    let summary = sales
        .entities
        .iter()
        .find(|entity| entity.name == "Order_Summary")
        .unwrap();
    assert!(!summary.persistable);
    assert_eq!(summary.source, Some(mxrs::EntitySourceDecl::Stored));
    let view = sales
        .entities
        .iter()
        .find(|entity| entity.name == "OrderTotal")
        .unwrap();
    assert!(!view.persistable);
    assert_eq!(
        view.source,
        Some(mxrs::EntitySourceDecl::OqlView {
            source_document: "Sales.OrderTotals".to_string(),
        })
    );
    // A view's attributes have no stored default to be authoritative about.
    assert_eq!(view.attributes[0].default_value, None);
}

#[test]
fn accessors_name_members_through_their_entity() {
    assert_eq!(Order::number().name(), "Number");
    assert_eq!(Order::number().qualified_name(), "Sales.Order.Number");
    assert_eq!(Order::customer().name(), "Order_Customer");
    assert_eq!(Order::watchers().qualified_name(), "Sales.Order_Watchers");
    assert_eq!(OrderSummary::qualified_name(), "Sales.Order_Summary");
    assert_eq!(Product::qualified_name(), "Catalog.Product");
    assert_eq!(Product::code().name(), "Code");
    assert_eq!(Availability::qualified_name(), "Catalog.Availability");
    let order = Var::<Order>::new("order");
    assert_eq!(
        order.get(Order::total()).gt(10.0).to_string(),
        "($order/Total > 10.0)"
    );
}

#[test]
fn an_enumeration_declares_its_values_and_captions() {
    let sales = sales();
    let status = &sales.enumerations[0];
    assert_eq!(status.name, "OrderStatus");
    assert_eq!(status.documentation, "Where an order stands.");
    let captions = status
        .values
        .iter()
        .map(|value| (value.name.as_str(), value.captions.clone()))
        .collect::<Vec<_>>();
    let caption = |language: &str, text: &str| (language.to_string(), text.to_string());
    assert_eq!(
        captions,
        [
            ("Open", vec![caption("en_US", "Open")]),
            ("Closed", vec![caption("en_US", "Closed order")]),
            (
                "on_hold",
                vec![caption("en_US", "On hold"), caption("nl_NL", "In de wacht")]
            ),
        ]
    );
}

#[test]
fn a_flow_is_named_by_its_prefix_and_function() {
    let sales = sales();
    let names = sales
        .microflows
        .iter()
        .map(|flow| flow.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        ["VAL_ValidateOrder", "ACT_CreateOrder", "Legacy_Cleanup"]
    );
    assert_eq!(
        VAL_ValidateOrder::qualified_name(),
        "Sales.VAL_ValidateOrder"
    );
    assert_eq!(NAN_OpenOrder::qualified_name(), "Sales.NAN_OpenOrder");
    assert_eq!(sales.nanoflows[0].name, "NAN_OpenOrder");
    // A flow kept in the imported model is nameable and declares nothing.
    assert_eq!(SUB_Audit::qualified_name(), "Sales.SUB_Audit");
    assert_eq!(return_::qualified_name(), "Sales.return");
    assert_eq!(NAN_Refresh::qualified_name(), "Sales.NAN_Refresh");
    assert_eq!(sales.nanoflows.len(), 1);
    // A hand-written declaration registers like a generated one.
    assert_eq!(sales.constants[0].name, "Region");
}

#[test]
fn security_and_navigation_are_declared_where_they_belong() {
    let project = Application::build();
    let sales = sales();
    let roles = sales.roles.as_ref().unwrap();
    assert_eq!(
        roles
            .iter()
            .map(|role| (role.name.as_str(), role.description.as_str()))
            .collect::<Vec<_>>(),
        [("Administrator", "Runs the shop."), ("Sales_Clerk", "")]
    );
    assert_eq!(Role::Clerk.qualified_name(), "Sales.Sales_Clerk");
    assert_eq!(sales.regular_expressions[0].name, "OrderNumberFormat");

    let security = project.security.as_ref().unwrap();
    assert_eq!(security.level, SecurityLevel::CheckEverything);
    assert_eq!(security.user_roles[0].module_roles, ["Sales.Administrator"]);
    // The demo user joins the security declared before it.
    assert_eq!(security.demo_users[0].name, "demo_manager");
    assert_eq!(security.demo_users[0].roles, ["Manager"]);
    // Only a project that declares no security carries its demo users
    // apart, for the writer to add to the security the model stores.
    assert!(project.demo_users.is_empty());

    let profile = &project.navigation.as_ref().unwrap().profiles[0];
    assert_eq!(profile.home_page.as_deref(), Some("Sales.Order_Overview"));
    // The item declared apart from the profile is appended to it.
    assert_eq!(profile.items.len(), 1);
    assert_eq!(
        profile.items[0].page.as_deref(),
        Some("Sales.Order_Overview")
    );
}

#[test]
fn documents_are_the_functions_that_build_them() {
    let sales = sales();
    let home = &sales.constants[1];
    assert_eq!(home.name, "HomeRegion");
    assert_eq!(home.documentation, "The shop's home region.");
    assert_eq!(home.value.as_deref(), Some("EU"));
    assert_eq!(sales.layouts[0].name, "ApplicationLayout");
    let page = &sales.pages[0];
    assert_eq!(page.name, "Order_Overview");
    assert_eq!(page.documentation, "Lists the open orders.");
    assert_eq!(page.title.as_deref(), Some("Orders"));
    assert_eq!(page.widgets.len(), 1);

    let create = &sales.microflows[1];
    assert_eq!(create.documentation, "Creates an order.");
    assert_eq!(create.parameters.len(), 2);
    assert_eq!(
        create.return_type,
        Some(mxrs::flow::FlowReturnType::Object(
            "Sales.Order".to_string()
        ))
    );
    assert_eq!(
        sales.microflows[0].return_expression.as_deref(),
        Some("not(isEmpty($Order/Number))")
    );
}
