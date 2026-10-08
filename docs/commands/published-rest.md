# `mxrs published-rest new`

```sh
mxrs published-rest new <Module.Handler> [--service NAME] [--path PATH] [--resource NAME]
    [--method get|post|put|patch|delete] [--operation-path PATH] [--query NAME]...
    [--target DIR] [--dry-run] [--json]
```

Publishes a REST operation: the handler microflow it calls, and the
operation in its service's declaration, `src/controllers/<module>/<service>.rs`
— the file the importer writes for a published REST service.

What is left unsaid comes from the handler's name. `MF_Order_Show` is a
`GET` of `order/{id}`: its verb says the method (`Create` posts, `Update`
puts, `Delete` deletes, anything else gets) and whether it addresses one
object (`{id}`), its subject the resource. The service is the module's only
one, else `<Module>Api` at `rest/<service>/v1`, published to anyone as
Studio Pro publishes a new service. Each `{name}` of the path and each
`--query` is a `String` parameter of the operation and of the microflow.

A service the module declares gains the operation, chained onto its
resource's operations; one it declares already at that method and path is
refused. In a project the importer gave an axum HTTP layer, the operation
is bound to a new function of the resource's controller, which signs the
caller in as the service asks and calls the microflow, and a new service's
router joins `controllers::router`. Any other project gets the declaration
alone, which every build writes into the model.

```rust
// src/controllers/sales/sales_api.rs, after
// `mxrs published-rest new Sales.MF_Order_Show` and `... MF_Order_List`
#[declaration(module = "Sales")]
pub fn sales_api(module: &mut ModuleBuilder) {
    let mut service = PublishedRestServiceBuilder::new("SalesApi", "rest/salesapi/v1");
    service.resource("order", |order| {
        order
            .get("{id}", "Sales.MF_Order_Show", |operation| {
                operation.path_parameter("id", RestParameterType::String);
            })
            .get("", "Sales.MF_Order_List", |_| {});
    });
    module.published_rest_service(service);
}
```
