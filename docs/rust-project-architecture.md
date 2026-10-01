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
presentation ─┐
              ├─> application ─> domain
infrastructure┘
```

- Domain owns business data, invariants, value objects and domain rules. It
  must not import Axum, a database driver or desktop framework.
- Application owns services and the ports they require. A Mendix server
  microflow is an application service unless it is proven to be a pure domain
  rule.
- Presentation translates HTTP, desktop commands or another delivery protocol
  into application calls. Framework request/response types stop at this edge.
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
│   ├── documents/<mendix_module>/<document>.rs
│   ├── module_security/<mendix_module>.rs
│   └── security.rs
├── application/
│   ├── services/<mendix_module>/<microflow>_service.rs
│   ├── services/<mendix_module>/imported.rs
│   ├── ports/<mendix_module>/{services,actions}.rs
│   └── task_queues.rs
├── presentation/
│   ├── http/
│   ├── pages/
│   ├── nanoflows/
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
is named in its module's `imported.rs` (`mxrs::imported! { ... }`), so the
rest of the project can still call and bind it. `mxrs::register!` registers
anything assembled directly against the project.

The crate root is therefore the four layers and the application, and no
file composes another:

```rust
pub mod application;
pub mod domain;
pub mod infrastructure;
pub mod presentation;

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
web UI outside it. Desktop commands are another presentation adapter; they call
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
