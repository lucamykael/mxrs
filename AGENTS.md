# MXRS engineering contract

This repository implements MXRS only. Treat the sibling MXRB repository as a
read-only behavioral oracle; never edit it.

## Product standard

- Editable generated Rust is a first-class product surface. It must be
  idiomatic, concise, expressive, discoverable, warning-free, and organized by
  clean architecture. Exploit Rust's type system to make invalid Mendix models
  difficult or impossible to express.
- MXRS is not a transliteration of MXRB. Where MXRB does something the Ruby
  way, MXRS does it the Rust way — and grows its own Rust-native surfaces:
  generated Mendix-to-Rust code leans on MXRS's own macros, DSL and style
  conventions, reads eloquently, and is formatted exactly as rustfmt would
  leave it. Parity of behavior, never parity of idiom.
- Zero known errors, silent loss, misleading diagnostics, opaque generated
  Rust, placeholder implementations, or untracked partial behavior is the
  completion standard. Preserve unsupported native data losslessly outside the
  editable Rust tree only as a temporary compatibility boundary, then add a
  typed DSL/IR/writer/exporter surface for it.
- Generated projects are layer-first for the modules the project created
  (user directive, 2026-09-29, superseding the module-first directive of
  2026-09-28): one `src/domain/`, one `src/services/`, one `src/ports/`, one
  `src/controllers/` and one `src/ui/` for the whole project, each concept —
  entities, DTOs, enumerations, export mappings, documents, module security;
  services; ports; controllers; pages, layouts, nanoflows — holding a folder
  per Mendix module. The module folder inside a concept is
  not a repetition of the layers: Mendix entity names are unique per module
  and not across a project, so it is what keeps them apart. Project-level
  layers keep cross-cutting concerns (project security, navigation, shared
  infrastructure). Dependencies point inward.
- Microflows are services, and services are a layer (user directive,
  2026-10-01): `src/services/<module>/`, beside `domain`, `controllers` and
  `infrastructure` — there is no `application` folder around it. A service
  is about something, and holds what the module does with it (user
  directive, 2026-10-02): `<subject>_service.rs` holds `pub struct
  <Subject>Service;` and a `#[service(module = "...", subject = ...)]`
  `impl` whose methods are the flows — `ACT_AssetType_Edit` is
  `AssetTypeService::edit`. The subject is the module's entity a flow's
  name names (its first word first; a verb such as `List` is what the flow
  does, not what it is about), else the word several flow names open with,
  else the module's own `<module>_service.rs`. The scaffold places a new
  flow where the importer would, from the entities and services the project
  already has, and names it by the subject the service's own attribute
  states; scaffolding a flow the service already declares is refused. There is no `imported.rs` listing
  leftovers. Every
  microflow body is editable Rust: what a typed builder cannot check is
  stated as Mendix writes it (`mx("...")`), and an activity no builder
  covers is stated as its document (`flow.native_action`) rather than
  keeping the flow out of Rust. A flow that still cannot be declared is
  named in its service's file, which says why. The attribute on a flow states what
  the flow is: its kind (`ACT`, `SUB`, ...), the roles that may run it
  (`roles(...)`, which is the model's own list), and what it calls, uses and
  is used by (`calls(...)`, `uses(...)`, `used_by(...)`), each named by the
  Rust item that declares it. Those three write nothing and are compared
  with the model on every build; a difference is a warning. `src/ports/`
  holds the contracts between the model and hand-written code, and task
  queues are declared in `src/services/task_queues.rs`.
- There is no `presentation` layer (user directive, 2026-10-01): what the
  application serves over HTTP and the user interface it declares are
  different things. `src/controllers/<module>/` holds, per published REST
  service, one file that is only its route table (`api_service.rs`: the
  axum `Router`, each path and method bound to a controller function) and
  one `<resource>_controller.rs` per resource with a function per
  operation, named by what it does to the resource (`index`, `show`,
  `create`, `update`, `destroy`) when that tells the operations apart and
  after the microflow each calls when it does not. Export mappings are
  part of the domain: `src/domain/mappings/<module>/`.
- The user interface is the frontend's (user directive, 2026-10-02):
  everything a user sees and runs in the browser lives in `frontend/`, a
  React + TypeScript project organized the way one is, and its TypeScript
  is the source a build reads into the model — mxrs parses it (oxc) and
  never runs it. Nanoflows are the frontend's services
  (`frontend/src/services/<module>/<subject>Service.ts`), pages are TSX
  (`frontend/src/pages/<module>/`), layouts are components
  (`frontend/src/components/layout/`), and the navigation is
  `frontend/src/navigation/index.ts`: `export default { profiles: [...] }
  satisfies Navigation;`, checked against `src/types/navigation.ts`. A
  declaration file is data — what would need running is refused with its
  line. A nanoflow is structured TypeScript (user directive, 2026-10-03): an
  `async` method of `nanoflowService("Module", { ... })`, its comment naming
  it (`@nanoflow ACT_Order_Open`, `@roles`, `@param`, `@mendixName`,
  `@defaultValue`), TypeScript's own `if`/`switch`/`for...of`/`return`/
  `break`/`continue` its control flow and an `await` per activity in the
  vocabulary of `src/mxrs/flows.ts` (`createObject`, `retrieve`,
  `changeList`, `callMicroflow`, `Service.method(...)` for another nanoflow,
  `showPage`, `onError`, ...) — read, never run, into the flow builder. The
  importer writes a nanoflow there only when its TypeScript reads back as
  the declaration its Rust body declares, and keeps it in Rust otherwise
  (`MXRS_EXPLAIN_FLOWS=1` says why); Rust pages call the frontend's
  nanoflows by the names `src/ui/nanoflows/<module>/in_frontend.rs` gives
  them (`mxrs::frontend_flows!`) — only the names Rust uses are there, and
  a flow relates to a nanoflow of the frontend (`calls`, `used_by`) by its
  qualified name, as it does to a page. A page, layout or snippet is TSX that
  states the document the model stores for it (`frontend/src/pages/<module>/`,
  `components/layout/<module>/`, `components/snippets/<module>/`): every
  stored document is an element named for its type (`Forms$DivContainer`
  is `<DivContainer>`), every field a prop, the widgets it holds its
  children, texts `{ en_US: "..." }` — nothing is outside the vocabulary,
  because the vocabulary is the model's own. `src/mxrs/elements.ts`, which
  the importer writes from the project's own documents and every build
  reads, says what each field holds when a page leaves it unsaid, so a
  page states only what differs; a type stored with another set of fields
  is an element of its own (`DivContainer_2`). An element mostly stated
  for one field says which in `elements.ts`, and a prop that holds it and
  says only that is the field's value: `captionTemplate={{ en_US: "Save" }}`,
  `attributeRef="Sales.Order.Number"`, `appearance="card"`. A pluggable
  widget's definition is declared once, in `src/widgets/<Name>.tsx`, with
  what each property holds before a use says anything, and used by that
  name with its properties by their keys — each the one thing a property
  of its type is for (`advanced: true`, `header: { en_US: "Name" }`,
  `columns: [{ ... }]`), or a `<WidgetValue>` when more is said. The importer declares a
  form there only when its TSX reads back as the stored document, and a
  build stores a form that says what is stored exactly as it is stored —
  identities and the order of a widget's properties kept. The project's
  own frontend build (Vite, `mxrs run --frontend`) runs the same file: a
  page is drawn from its TSX inside the layout it calls, under the
  project's compiled theme, each element the way `src/components/elements`
  draws its stored type and one nobody draws yet as a box holding its
  children; a list shows the objects of its entity, a data view the object its
  page was given, their inputs fill it, and a button opens a page with the
  object it is in, creates, saves or deletes one through the runtime's
  `/api/data/<operation>` — each member read or written within the rights
  of the caller's role, each value checked against what its attribute
  holds, a save held to the entity's validation rules (what is refused is
  shown under its input) and committed through its event handlers as a
  flow's commit is; a list asks for its XPath constraint, sort order and
  page — or runs its flow with the object it is in, applying what the flow
  asks of the page. The bundle mxrs embeds holds no project's
  pages and draws what the manifest describes. The importer,
  `mxrs new` and the scaffolds write these files: a scaffolded page or
  layout is built as what it is, made the document the model would store,
  and stated with the project's elements — one the project lacks is added
  to `elements.ts` after those it has, whose defaults stay what its pages
  were written against — and a templated page adds its item to the
  frontend's navigation. A page of the frontend calls a nanoflow by its
  name, so nothing names a scaffolded nanoflow in Rust; `src/ui/` is what
  a project imported earlier still declares there (a Rust page, the
  `in_frontend.rs` names such pages call nanoflows by, a nanoflow or a
  navigation its TypeScript could not restate), and a fresh project has
  none: the layer is declared when it is there, and only then.
- A declaration is one annotated item in its own file, and it registers
  itself (user directive, 2026-10-01). `#[entity]`/`#[dto]`/`#[view]` on a
  struct, `#[enumeration]`/`#[module_roles]` on an enum, and `#[microflow]`/
  `#[nanoflow]`/`#[page]`/`#[layout]`/`#[constant]`/`#[security]`/
  `#[navigation]` — or `#[declaration]` for what none of those cover — on the
  function that builds it. The importer, `mxrs new` and every scaffold write
  these forms, and adding an artifact is the file plus its `pub mod` line:
  no aggregator, no `apply` chain, no composition root. Generated source
  never restates what a macro already assumes, never carries a Mendix
  identifier, and never falls back to an imperative IR dump where a
  declaration can say it: a name Rust cannot spell is spelled differently and
  stated with `name = "..."`, and what a declaration genuinely cannot restate
  is kept from the imported model with an explicit `preserve(...)`. What
  names an artifact lives with it — an attribute is its entity's accessor
  (`Order::number()`), a flow is the type its declaration generates — so
  authored modules have no separate marker file.
- An activity of a flow is the macro named for it (user directive,
  2026-10-02): `create_object!`, `change_object!`, `commit_object!`,
  `delete_object!`, `rollback_object!`, `retrieve!`, `create_list!`,
  `change_list!` (the list itself — `add`, `remove`, `replace`, `clear` —
  or another made from it — `sort`, `filter`, `find_by`, `head`, `union`,
  ... with its `name`), `aggregate_list!`, `create_variable!`,
  `change_variable!`, `call_microflow!`, `call_nanoflow!`,
  `call_javascript_action!`, `log!`, `show_page!`, `close_page!`,
  `show_message!` — `retrieve!(flow, &order,
  by = Order::customer(), name = "...")` over an association. A member,
  attribute or association, is its struct field. Their arguments are Rust
  expressions, so rustfmt lays them out: `create_object!(flow, Order {
  number: "A-1", status: OrderStatus::Open, customer: customer }, commit)`.
  A value is the Mendix expression it reads as — a string literal or text
  constant a Mendix string, a number or boolean itself, a variable `$` and
  its name, an enumeration variant its qualified value (through
  `ActivityValue`) — and `mx("...")` is what Rust cannot check. A field set
  to its namesake variable is written alone (`Order { customer }`).
  Contradictory options (`first` with a `range`, two aggregate functions)
  do not compile. A name Studio Pro would give by default
  (`New<Entity>`, `<Entity>List`) is not stated. Each macro expands to the
  builder call the activity is, and the importer writes a macro wherever
  every part of the activity reads back exactly, the builder call
  otherwise.
- A Java action runs its Java in Mendix and Rust in mxrs (user directive,
  2026-10-02); mxrs embeds no JVM and translates no Java. The Java is the
  project's own code, editable in `java/` with Mendix's layout, and every
  build ships it as the `.mpr`'s `javasource/`. Every Java action
  of the model, a package's included, has a contract in its module's
  `ports::actions` — a trait whose documentation shows the Java the action
  runs in Mendix — a `register_*` function in
  `infrastructure::generated::<module>_actions`, and a macro named for it
  that flows call it by: `execute_oql_statement!(flow, "Result", statement
  = ..., returnEntity = SPCProgramView)` (the generic form is
  `call_java_action!`). An entity an action takes is its struct. A type no
  struct names is still carried: `MxAnyObject`/`ObjectRef`,
  `MxEntityName`, `MxValue`. mxrs implements some Marketplace actions itself
  (`FlowEngine::with_marketplace_java_actions`, each following its Java —
  its edge cases included, tested against what the Java returns); the
  project registers its own in `infrastructure::adapters::java_actions`,
  which replace mxrs's; and every runtime that boots flows (the generated
  one, `mxrs run`) names, by module, each action still without a Rust
  implementation.
- Generated Rust reads in paragraphs (user directive, 2026-10-02): a
  declaration's attributes sit directly above its `pub fn`; a flow's
  parameters, return type and one-line `let`s are stacked; every other
  operation has a blank line before and after it, while one activity's
  options stay together; a blank line separates functions. Text too long
  for its line, or written over several lines (an XPath, a long
  expression), is a named `const` at the top of the file — rustfmt gives up
  on a whole statement holding a literal it cannot fit.
- Modules the project *installed* are module-first, under
  `src/packages/<module>/` (user directive, 2026-09-29):
  `Projects$Module`'s `FromAppStore` decides which, and a package carries only
  its types and contracts plus any REST surface the application serves. It
  declares nothing — no `apply` anywhere in that tree — because an edit there
  is lost the next time the module is upgraded, and `model/imported` stays its
  only source of truth.
- The scaffold and the importer write the same folders. A scaffolded concept
  and an imported one are the same file in the same place; a divergence
  between them is a defect, not a difference in taste.
- Scaffolds must compile immediately, be transactional, refuse overwrites, and
  be configurable through explicit command options and project defaults.
  `mxrs page new <Module.Entity> --template crud` writes the overview and
  edit pages of an entity the project declares, from its attributes — a
  list with a way to create, edit and delete, and a form with a labelled
  input per attribute — and adds the overview to the navigation; what the
  pages leave out (an association, a binary) is said. It is the first
  screen of an application developed with mxrs alone, not all of one: its
  pages work on the entity itself, and a flow of the project is not called
  with arguments yet.
- Never commit private acceptance projects, their names, paths, metrics, or
  identifying references. Tests and documentation use neutral corpus names and
  environment variables. Public fixtures must be redistributable.
- Never commit credentials. Platform access commands store only references to
  secrets and enforce private filesystem permissions.

## Increment protocol

1. Read `decisions/mxrs-open-backlog.md` from the current project's ai-memory
   before non-trivial work. Update that page in place; do not append competing
   status snapshots.
2. The implementing agent changes code. A separate Claude review agent owns
   tests and review for each increment. The reviewer must not edit production
   code unless explicitly assigned a separate implementation front.
3. Exchange status and findings through the current project's ai-memory. Never
   copy credentials or private-corpus identifiers into memory.
4. Accept an increment only after formatting, focused tests, full workspace
   tests, clippy with warnings denied, generated-source noise audit, capability
   matrix, and relevant round-trip/oracle checks pass.
5. Commit one coherent, validated increment with a clean worktree. Update the
   pinned backlog to the accepted commit and exact gate counts.

## ai-memory routing

Use `ai-memory` from the repository root without `--project` or `--workspace`,
so it remains scoped to MXRS. Read full relevant decision/rule/gotcha pages,
not only search snippets. Durable decisions and handoffs belong in ai-memory;
the repository contains only product documentation and these operating rules.
