# Rust project architecture used by MXRS

This document records the engineering baseline used when MXRS turns a Mendix
application into editable Rust. It is a synthesis, not a copy of the referenced
books. The links are the authority when this document and an upstream source
disagree.

## Sources reviewed

- [The Rust Programming Language: packages, crates and modules](https://doc.rust-lang.org/book/ch07-00-managing-growing-projects-with-packages-crates-and-modules.html)
- [The Cargo Book: workspaces](https://doc.rust-lang.org/cargo/reference/workspaces.html)
- [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/checklist.html)
- [Rust Design Patterns](https://rust-unofficial.github.io/patterns/)
- [Clippy lint catalogue](https://rust-lang.github.io/rust-clippy/stable/index.html)
- [rustfmt documentation](https://rust-lang.github.io/rustfmt/)
- [axum examples](https://github.com/tokio-rs/axum/tree/main/examples)
- [Tokio mini-redis](https://github.com/tokio-rs/mini-redis)
- [Zero To Production In Rust reference project](https://github.com/LukeMathWalker/zero-to-production)
- [rust-analyzer workspace](https://github.com/rust-lang/rust-analyzer)
- [Tauri project structure](https://tauri.app/start/project-structure/)

## Conclusions

Rust prescribes Cargo conventions and a module system, not one universal
application folder tree. Structure should expose ownership and dependency
direction. Split files when a concept has independent reasons to change; split
crates only for a real compilation, ownership, reuse or deployment boundary.
Deep ceremonial trees and a crate per layer add navigation and build cost
without increasing encapsulation.

For a converted Mendix application, a single library-plus-thin-binary package
is the default. The library makes application behavior testable without
starting a process. `main.rs` parses process concerns and delegates. Large
systems may graduate to a workspace when they gain independently deployable
services or genuinely reusable libraries.

Dependencies point inward:

```text
controllers ──┐
ui ───────────┼─> services ─> domain
infrastructure┘   (ports)
```

- Domain owns business data, invariants, value objects and domain rules. It
  must not import Axum, a database driver or desktop framework.
- Services are what the application does. Every Mendix microflow is a
  service: a layer of its own beside the domain, one folder per module and
  one file per microflow. There is no `application` folder wrapping it.
- Ports are the contracts between the model and hand-written code: what a
  module's services offer to callers, and what its actions need an adapter
  to provide.
- Controllers translate HTTP — or desktop commands, or another delivery
  protocol — into service calls. Framework request/response types stop at
  this edge.
- UI is the user interface the model declares: pages, layouts, nanoflows and
  navigation. It is not where the application's HTTP surface lives.
- Infrastructure implements ports for persistence, runtimes, queues and
  external services.
- Composition of the *model* needs no module at all: declarations register
  themselves. What assembles concrete runtime adapters stays explicit, in
  `infrastructure`, which avoids service locators for behavior.

## Generated layout

```text
src/
├── lib.rs
├── main.rs
├── domain/
│   ├── entities/<mendix_module>/<entity>.rs
│   ├── dtos/<mendix_module>/<dto>.rs
│   ├── enumerations/<mendix_module>/<enumeration>.rs
│   ├── mappings/<mendix_module>/<export_mapping>.rs
│   ├── documents/<mendix_module>/<document>.rs
│   ├── module_security/<mendix_module>.rs
│   └── security.rs
├── services/
│   ├── <mendix_module>/<subject>_service.rs   one per subject
│   └── task_queues.rs
├── ports/
│   └── <mendix_module>/{services,actions}.rs
├── controllers/
│   ├── mod.rs                              router() and serve()
│   ├── state.rs, error.rs
│   └── <mendix_module>/
│       ├── <published_service>.rs          the route table
│       └── <resource>_controller.rs        one function per operation
├── ui/
│   ├── pages/<mendix_module>/<page>.rs
│   ├── layouts/<mendix_module>/<layout>.rs
│   ├── nanoflows/<mendix_module>/<nanoflow>.rs
│   └── navigation.rs
├── infrastructure/
│   ├── adapters/
│   └── persistence.rs
└── packages/<marketplace_module>/...
```

The outer concept-first tree prevents a Studio Pro module from becoming a
miniature copy of the whole architecture. A Mendix module remains a namespace
inside a concept because Mendix names are only unique per module. Marketplace
modules are the exception: they are external upgrade units and therefore stay
module-shaped under `packages/`.

## Services

A service is about something, and holds what its module does with it.
Every microflow of a module the project created is a method of the service
of its subject, under `src/services/<module>/`: `ACT_Order_Create`,
`SUB_Order_Number` and `DS_List_Order` are `create`, `number` and `list` of
`OrderService`, in `order_service.rs`. The subject is the module's entity
the flow's name names — its first word first, since a word such as `List`
says what the flow does rather than what it is about — or else the word
several of the module's flow names open with (`MF_RubyCrud_Create` →
`RubyCrudService`). Whatever no subject gathers is a method of the module's
own service, `sales_service.rs`.

The `#[service]` attribute gives every method its module, and names each
flow `KIND_Subject_Method`; a flow whose model name says it differently
states it with `name = "..."`. The attribute on each method says what the
flow is beside what it does:

```rust
// src/services/sales/order_service.rs
use mxrs::prelude::*;

use crate::domain::entities::sales::order::Order;
use crate::domain::module_security::sales::Role;
use crate::ui::nanoflows::sales::order_save::ACT_Order_Save;

/// What the Sales module does with Order.
pub struct OrderService;

#[service(module = "Sales", subject = Order)]
impl OrderService {
    #[microflow(
        ACT,
        roles(Role::Administrator, Role::Clerk),
        calls(SUB_Order_Number),
        uses(Order),
        used_by(ACT_Order_Save, "Sales.Order_Overview")
    )]
    pub fn create(flow: &mut FlowBuilder) {
        let number = flow.call_into("Number", MicroflowRef::<SUB_Order_Number>::new(), |_| {});

        let order = flow.create("NewOrder", Ref::<Order>::new(), |create| {
            create.set(Order::number(), mx("$Number"));
            create.commit(Commit::Yes);
        });

        flow.return_with(mx("$NewOrder"));
    }

    #[microflow(SUB)]
    pub fn number(flow: &mut FlowBuilder) {
        flow.returns(DataType::String);

        flow.return_with(mx("'ORD-' + toString(dateTimeToEpoch([%CurrentDateTime%]))"));
    }
}
```

- The first word is the flow's kind — the prefix its name carries (`ACT`,
  `SUB`, `DS`, `VAL`, ...).
- `roles(...)` are the module roles that may run it, as variants of the enum
  the module declares its roles with. This is the model's own list: stating
  it writes it, stating `roles()` allows nobody, and leaving it out keeps
  what the model has.
- `calls(...)`, `uses(...)` and `used_by(...)` name the flows it calls, the
  entities it works with and what refers to it. Each item is the Rust item
  that declares the thing — the type a flow's declaration generates in
  `calls(...)` and `used_by(...)`, an entity's struct in `uses(...)` — so
  the compiler checks it exists and an editor goes to its file; what no Rust
  item declares, a page or an entity in `used_by(...)` for instance, is named
  as the model names it. They write nothing: the body and the documents that refer
  to the flow already say all of it. A build compares the two and reports
  each difference as a warning, so the lists stay true or say where they are
  not. A relation left out is not compared.

A flow the importer cannot declare yet is named in its service's file all
the same, so the rest of the project can call and bind it, with a comment
saying why it stayed in the imported model:

```rust
//! `Sales.SUB_Legacy` stays in the imported model: its module has two flows of that name.

mxrs::imported! {
    module = "Sales";
    microflow SUB_Legacy;
}
```

`MXRS_EXPLAIN_FLOWS=1 mxrs convert mendix-to-rust ...` prints the same
reasons while importing.

The body is written with the flow builder. Where a typed builder can check
an expression it does (`Order::number().set(number)`); everything else is
stated as Mendix writes it, with `mx("...")`. What the block structure alone
does not say has a builder of its own:

| In the model | In Rust |
|---|---|
| Custom error handling on an activity | `flow.on_error(\|flow\| ...)`, `on_error_without_rollback`, `continue_on_error()` before the activity; `flow.raise_error()` |
| A split per enumeration value / per entity | `flow.switch(mx(..), \|on\| ...)`, `flow.switch_type(&var, \|on\| ...)` with `flow.cast(..)` |
| A split a rule decides | `flow.decision_by_rule(..)`, `flow.switch_by_rule(..)` |
| An activity kept but disabled | `flow.disabled()` before it |
| A merge the flow returns to (a retry) | `flow.label("again")` and `flow.jump("again")` |
| A call on a task queue, or one whose result is dropped | `call.queue(..)`, `call.discard_result(..)` |
| An activity no builder covers yet | `flow.native_action(NativeDocument::new(..).with(..))` |

An edit that keeps the flow's structure changes only what it says: node
identities, positions, captions and annotations stay the model's. An edit
that changes the structure rebuilds the graph: its annotations are kept,
without the lines that attached them to activities, while activity
captions, documentation, colours and layout start over — and the build says
so for each flow it rebuilds.

## Controllers

A published REST service is two kinds of file in its module's controllers
folder. The service file is the route table and nothing else — the axum
`Router`, every path the model publishes, each method bound to the function
that handles it — together with what the model says about who may call:

```rust
// src/controllers/sales/order_service.rs
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/orders", get(orders_controller::index).post(orders_controller::create))
        .route(
            "/api/v1/orders/{id}",
            get(orders_controller::show)
                .put(orders_controller::update)
                .delete(orders_controller::destroy),
        )
}
```

Each resource of the service is a controller, `<resource>_controller.rs`,
with one function per operation calling the microflow the model bound to
it. A function is named by what the operation does to the resource —
`index`, `show`, `create`, `update`, `destroy` — when that says which
operation it is; operations that would share one of those names are named
after the microflow each calls. The project's `controllers/mod.rs` merges
every route table behind the shared `AppState`.

The JSON document an operation answers with is an export mapping, and a
mapping describes the domain's data, not the transport: it lives in
`src/domain/mappings/<module>/` and the controller imports it.

## Frontend

`frontend/` is a React + TypeScript + Vite project, laid out the way one
is, so the person opening it finds things where a React project keeps them:

```text
frontend/
├── index.html
├── package.json, tsconfig.json, vite.config.ts
├── scripts/sync-assets.mjs
└── src/
    ├── main.tsx                 the entry point
    ├── App.tsx                  the shell: header, navigation, current page
    ├── api/                     calls to the Rust runtime (model, actions)
    ├── components/
    │   ├── layout/              header and navigation
    │   └── widgets/             one renderer per widget of the model
    ├── hooks/                   the loaded model, the hash route
    ├── pages/                   what a route shows
    ├── types/                   the model as the runtime publishes it
    ├── utils/                   pure helpers
    └── styles/                  base, layout and widget CSS
```

Imports name `src/` as `@/` (`tsconfig.json` declares it and Vite reads it
from there). The production bundle `mxrs` ships is built from exactly these
files, and its hash is pinned against them.

## Declaring the model

A declaration is one annotated item in its own file. It registers itself with
the application, so adding an artifact is the file plus its `pub mod` line —
there is no aggregator to edit and no composition call to add.

```rust
use mxrs::prelude::*;

use crate::domain::entities::sales::customer::Customer;

/// A customer order.
#[entity(module = "Sales")]
#[mxrs(index(number))]
#[mxrs(before_commit = "Sales.VAL_Order")]
pub struct Order {
    /// Printed on the invoice.
    #[mxrs(length = 80, required, unique)]
    pub number: MxString,
    pub total: MxDecimal,
    pub customer: Reference<Customer>,
}
```

| Declaration | On | Declares |
| --- | --- | --- |
| `#[entity(module = "...")]` | struct | a persistable entity |
| `#[dto(module = "...")]` | struct | a non-persistable entity |
| `#[view(module = "...", source = "...")]` | struct | an OQL view entity |
| `#[enumeration(module = "...")]` | enum | an enumeration; variants are its values |
| `#[microflow(ACT, module = "...")]` | fn | a server-side microflow |
| `#[nanoflow(NAN, module = "...")]` | fn | a client-side nanoflow |
| `#[page(module = "...")]` | fn | a page |
| `#[layout(module = "...")]` | fn | a page layout |
| `#[constant(module = "...")]` | fn | a constant |
| `#[menu(module = "...")]` | fn | a standalone menu |
| `#[declaration(module = "...")]` | fn | anything else a module holds, through its `ModuleBuilder` |
| `#[module_roles(module = "...")]` | enum | a module's roles; variants are the roles |
| `#[security]` | fn | the project's security |
| `#[navigation]` | fn | the project's navigation profiles |
| `#[navigation_item(profile = "...", caption = "...")]` | fn | one item, declared next to the page it opens |
| `#[demo_user]` | fn | a local demo user |

The rules that keep these readable:

- **The type says what it can.** A field's type is the attribute's
  (`MxString`, `MxDecimal`, `MxBool`, an enumeration's own enum) or the
  association's (`Reference<T>`, `ReferenceSet<T>`). `#[mxrs(...)]` states
  only what a type cannot: `length`, `required`, `unique`, `default`,
  `index(...)`, an event handler, the parent an entity specializes
  (`generalizes = Document`, `generalizes = mxrs::system::User`) or the
  system members a root keeps (`stores(owner, created_date, changed_date,
  changed_by)`).
- **Defaults are not restated.** A string is 200 characters, a date is
  localized, a boolean defaults to `false` and a number to `0` unless the
  field says otherwise — the platform's own defaults. Deleting an option
  returns the attribute to that default, not to whatever an imported model
  held. An entity that names no parent and stores no system member is a
  plain root, by the same rule. `no_default` and
  `preserve(indexes|lifecycle|image|inheritance)` are the explicit
  exceptions, for a model that genuinely differs or holds something the
  declaration has no word for.
- **`///` is the documentation.** A doc comment on a struct, field, variant
  or function is the Mendix documentation of what it declares.
- **A name is where the thing is.** An attribute is its entity's accessor:
  `Order::number().set("A-1")`, `order.get(Order::total())`,
  `view.text_box(Order::number())`. A flow is the type its declaration
  generates, under its Mendix name: `fn create_order` declared with
  `#[microflow(ACT, ...)]` is `Sales.ACT_CreateOrder`, named elsewhere as
  `ACT_CreateOrder`. There is no separate marker file to keep in step.
- **A Rust name that cannot spell the Mendix name states it.** `name =
  "..."` on the declaration, `#[mxrs(name = "...")]` on a field.

A flow the project keeps in its imported model without declaring it in Rust
still has the file its declaration would have, naming it
(`mxrs::imported! { ... }`) so the rest of the project can call and bind it. `mxrs::register!` registers
anything assembled directly against the project.

The crate root is therefore the layers and the application, and no file
composes another:

```rust
pub mod controllers;
pub mod domain;
pub mod infrastructure;
pub mod ports;
pub mod services;
pub mod ui;

#[mxrs::application(version = "11.12.1")]
pub struct Application;
```

A layer or concept `mod.rs` is a list of `pub mod` lines and nothing else.
The order declarations are assembled in does not depend on the order they
are linked or listed in: it is fixed by what each one declares and where it
is written, so the same source builds the same model everywhere.

## Identity

Generated source carries no Mendix identifier, and none is written by hand.
An artifact that came from an imported model keeps the identity
`model/imported/` records for it, matched by qualified name. A new artifact
gets an identity derived from the project and its kind and qualified name:
stable across builds and machines, and never random. The consequence is that
renaming an artifact in Rust declares a new one rather than renaming the
imported one.

## API and Axum rules

- Build routers from small functions and merge/nest them at the HTTP boundary.
- Put shared dependencies in an explicit, cheaply cloneable state value.
- Convert application errors to HTTP in one boundary error type implementing
  `IntoResponse`; do not leak internal details in 5xx responses.
- Keep extractors, headers, status codes and JSON envelopes out of domain and
  application code.
- Use Tokio's async entry point only in the binary/boundary. Do not make pure
  domain operations async merely because the server is async.
- Add tracing, timeouts, body limits and request identifiers at the router
  middleware boundary when the generated runtime supports them.

## Services, microservices and desktop

A service remains one package while it is one deployment unit. A microservice
system should use a virtual Cargo workspace with one package per independently
deployable service and shared crates only for stable, intentionally shared
contracts. Sharing all internal models couples deployments and defeats the
boundary.

For desktop, Tauri keeps the Rust package under `src-tauri/` and the optional
web UI outside it. Desktop commands are another delivery adapter; they call
the same application use cases as HTTP and must not move framework types into
the core. MXRS currently generates server delivery modes, so desktop guidance
is architectural input rather than a promised conversion target.

## Idiomatic and quality baseline

- Follow Rust casing: modules/functions/locals are `snake_case`; types and
  traits are `UpperCamelCase`; constants are `SCREAMING_SNAKE_CASE`.
- Prefer domain types that make invalid states hard to construct. Validate at
  boundaries and return `Result` for recoverable failures.
- Keep visibility narrow. Public APIs need useful rustdoc; implementation
  details remain private.
- Prefer standard conversion traits (`From`, `TryFrom`, `AsRef`) and common
  traits (`Debug`, `Clone`, equality/hash/order where meaningful).
- Avoid `unwrap`/`expect` in request and persistence paths. A poisoned lock or
  invariant violation needs an explicit policy.
- Generated packages declare an MSRV, forbid unsafe code, enable Clippy's
  `all` group, format with rustfmt, and are checked with tests and Clippy.
- Tests follow risk boundaries: unit tests for pure transformations,
  integration tests for public behavior, and end-to-end conversion tests for
  Mendix → Rust → Mendix fidelity.

## Deliberate non-rules

“Clean Architecture”, “hexagonal”, “DDD” and “vertical slice” are vocabulary,
not Cargo features. MXRS uses only the parts that create enforceable dependency
boundaries. It does not generate empty directories, generic `utils`, one trait
per function, or a workspace/crate per layer. Those patterns are added only
when concrete code needs them.
