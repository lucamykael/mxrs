//! An activity macro says what the builder call it expands to says: the
//! same flow written either way declares the same activities.

use mxrs::prelude::*;

#[enumeration(module = "Sales", name = "ENUM_Status")]
pub enum OrderStatus {
    Open,
    #[mxrs(name = "Closed_Value")]
    Closed,
}

#[entity(module = "Sales")]
pub struct Customer {
    pub name: MxString,
}

#[entity(module = "Sales")]
pub struct Order {
    pub number: MxString,
    pub total: MxDecimal,
    pub count: MxInteger,
    pub paid: MxBool,
    pub status: OrderStatus,
    pub customer: Reference<Customer>,
}

#[microflow(SUB, module = "Sales")]
pub fn ship(_flow: &mut FlowBuilder) {}

#[microflow(ACT, module = "Sales")]
pub fn with_macros(flow: &mut FlowBuilder) {
    let customer = flow.parameter_of("Customer", DataType::object::<Customer>(), |_| {});

    let order = create_object!(
        flow,
        Order {
            number: "O'Brien",
            total: 10.50,
            count: -3,
            paid: true,
            status: OrderStatus::Closed,
            customer: customer,
        },
        commit,
        refresh
    );

    create_object!(flow, Order {}, name = "Spare");

    change_object!(
        flow,
        &order,
        Order {
            status: OrderStatus::Open
        },
        commit_without_events
    );

    commit_object!(flow, &order, without_events, refresh);

    let orders = retrieve!(
        flow,
        Order,
        xpath = "[Paid]",
        sort = [("Sales.Order.Number", Descending)],
        range = (10, 0)
    );

    retrieve!(flow, Order, first);

    let list = create_list!(flow, Order);

    change_list!(flow, &list, add = order);

    change_list!(flow, &list, clear);

    change_list!(
        flow,
        &orders,
        filter = (Order::paid(), true),
        name = "PaidOrders"
    );

    change_list!(
        flow,
        &orders,
        find_by = mx("$currentObject/Total > 5"),
        name = "Large"
    );

    change_list!(
        flow,
        &orders,
        sort = [(Order::number(), Ascending)],
        name = "Sorted"
    );

    change_list!(flow, &orders, union = &list, name = "All");

    change_list!(flow, &orders, head, name = "FirstOrder");

    aggregate_list!(flow, &orders, count, name = "Count");

    aggregate_list!(flow, &orders, sum = Order::total(), name = "Sum");

    aggregate_list!(
        flow,
        &orders,
        average_of = mx("$currentObject/Total"),
        name = "Average"
    );

    let counter = create_variable!(flow, DataType::Long, 0, name = "Counter");

    change_variable!(flow, &counter, mx("$Counter + 1"));

    call_microflow!(flow, SUB_Ship { Order: order }, name = "Shipped");

    call_microflow!(flow, SUB_Ship);

    log!(
        flow,
        Info,
        "Orders",
        "{1} shipped",
        parameters = [counter],
        stack_trace
    );

    delete_object!(flow, &order, refresh);

    rollback_object!(flow, &order);
}

#[microflow(ACT, module = "Sales")]
pub fn with_builders(flow: &mut FlowBuilder) {
    let customer = flow.parameter_of("Customer", DataType::object::<Customer>(), |_| {});
    let order = flow.create("NewOrder", Ref::<Order>::new(), |create| {
        create.set(Order::number(), mx("'O''Brien'"));
        create.set(Order::total(), mx("10.50"));
        create.set(Order::count(), mx("-3"));
        create.set(Order::paid(), mx("true"));
        create.set(Order::status(), mx("Sales.ENUM_Status.Closed_Value"));
        create.set(Order::customer(), &customer);
        create.commit(Commit::Yes);
        create.refresh_in_client(true);
    });
    flow.create("Spare", Ref::<Order>::new(), |_| {});
    flow.change(&order, |change| {
        change.set(Order::status(), mx("Sales.ENUM_Status.Open"));
        change.commit(Commit::WithoutEvents);
    });
    flow.commit_with(&order, |commit| {
        commit.with_events(false);
        commit.refresh_in_client(true);
    });
    let orders = flow.retrieve("OrderList", Ref::<Order>::new(), |retrieve| {
        retrieve.xpath("[Paid]");
        retrieve.sort_by("Sales.Order.Number", SortOrder::Descending);
        retrieve.range(mx("10"), mx("0"));
    });
    flow.retrieve("Order", Ref::<Order>::new(), |retrieve| {
        retrieve.first();
    });
    let list = flow.create_list_of("OrderList", Ref::<Order>::new());
    flow.change_list(&list, ListChange::Add, mx("$NewOrder"));
    flow.change_list(&list, ListChange::Clear, mx(""));
    flow.list_filter("PaidOrders", &orders, Order::paid(), mx("true"));
    flow.list_find_by("Large", &orders, mx("$currentObject/Total > 5"));
    flow.list_sort("Sorted", &orders, |sort| {
        sort.by(Order::number(), SortOrder::Ascending);
    });
    flow.list_union("All", &orders, &list);
    flow.list_head("FirstOrder", &orders);
    flow.aggregate("Count", &orders, AggregateFunction::Count, |_| {});
    flow.aggregate("Sum", &orders, AggregateFunction::Sum, |aggregate| {
        aggregate.attribute(Order::total());
    });
    flow.aggregate(
        "Average",
        &orders,
        AggregateFunction::Average,
        |aggregate| {
            aggregate.expression(mx("$currentObject/Total"));
        },
    );
    let counter = flow.create_variable("Counter", DataType::Long, mx("0"));
    flow.change_variable(&counter, mx("$Counter + 1"));
    flow.call_into("Shipped", MicroflowRef::<SUB_Ship>::new(), |call| {
        call.argument("Order", mx("$NewOrder"));
    });
    flow.call(MicroflowRef::<SUB_Ship>::new(), |_| {});
    flow.log(LogSeverity::Info, mx("'Orders'"), "{1} shipped", |log| {
        log.parameter(mx("$Counter"));
        log.include_stack_trace(true);
    });
    flow.delete_with(&order, |delete| {
        delete.refresh_in_client(true);
    });
    flow.rollback_with(&order, |_| {});
}

#[mxrs::application(version = "11.12.1")]
pub struct Application;

fn flow<'a>(project: &'a mxrs::ProjectDecl, name: &str) -> &'a mxrs::MicroflowDecl {
    project
        .modules
        .iter()
        .find(|module| module.name == "Sales")
        .expect("the Sales module is declared")
        .microflows
        .iter()
        .find(|flow| flow.name == name)
        .unwrap_or_else(|| panic!("{name} is declared"))
}

#[test]
fn an_activity_macro_declares_what_its_builder_call_does() {
    let project = Application::build();
    let macros = flow(&project, "ACT_WithMacros");
    let builders = flow(&project, "ACT_WithBuilders");
    assert_eq!(
        format!("{:?}", macros.parameters),
        format!("{:?}", builders.parameters)
    );
    assert_eq!(macros.activities.len(), builders.activities.len());
    for (index, (macro_activity, builder_activity)) in macros
        .activities
        .iter()
        .zip(&builders.activities)
        .enumerate()
    {
        assert_eq!(
            format!("{macro_activity:?}"),
            format!("{builder_activity:?}"),
            "activity {index}"
        );
    }
}
