# mxrs

A ground-up Rust implementation of Mendix Studio Pro `.mpr` project reading,
writing, compiling and packaging — no Ruby or mxcli runtime dependency.

Sibling project to [mxrb](https://github.com/lucamykael/mxrb) (Ruby), which
remains the reference implementation and behavioral oracle during
development, but is not a runtime dependency of anything shipped here.

The full phased implementation plan (crate layout, dependency DAG, milestone
done-definitions, DSL/macro design, testing strategy) lives in this
project's `ai-memory` as `decisions/mxrs-rust-rewrite-plan.md`.

## Status

`mxrs modules FILE.mpr` lists module names and entity/page/microflow counts.
Use `--json` for structured output or `--names` for the previous sorted list.
The [modules command contract](docs/commands/modules.md) documents its MXRB
verification and how to repeat the comparison.

`mxrs dump-unit FILE.mpr UNIT_ID` prints stored unit metadata and original
bytes in native hexadecimal/ASCII format. Its [command contract](docs/commands/dump-unit.md)
includes byte-for-byte CLI verification against MXRB.

The read-command contracts now also cover [SQL](docs/commands/sql.md),
[native inspection via `units`](docs/commands/inspect.md),
[protocol audit](docs/commands/protocols.md), [MDA inspection/comparison](docs/commands/mda.md)
and [Cargo workspace inspection](docs/commands/project.md). Each includes a
repeatable MXRB CLI oracle and explicit presentation or language mappings.
Protocol audit remains GUID-based and fail-closed; neither current registry
contains evidenced recognition GUIDs.

33 crates + a dev-only `xtask` harness. The primary direction is Cargo-native: Rust is the editable
source of truth and `.mpr` is an import/export build artifact.

```text
existing .mpr -> mxrs import -> Cargo project -> cargo check/test
                                          \-> cargo run -> new .mpr
```

An import writes model declarations under `src/domain/`, server-side use cases
and microflows under `src/application/`, pages, navigation, and client-side
nanoflows under `src/presentation/`, and outbound/generated adapters under
`src/infrastructure/`. A complete unit snapshot stays under `model/imported/`.
Stable identities and storage metadata remain data instead of noisy Rust
constants. Mendix filesystem resources are copied into the editable `assets/`
tree and materialized next to each built `.mpr`. The snapshot makes concepts
without a friendly Rust representation lossless; rebuilding does not read or
patch the original `.mpr`. Typed coverage can therefore replace snapshot
content incrementally without blocking a correct build.

The crates are grouped by dependency layer under `crates/{io,model,authoring,
writer,compiler,app}/`; package names and public APIs are unchanged. See
[`crates/README.md`](crates/README.md) for the ownership boundary.

Two lower-level compiler pipelines share only the compiled model as input:

**Writer path** (DSL definition → `.mpr`, what Studio Pro opens):
`mxrs-bson` → `mxrs-mpr` / `mxrs-fragment-store` / `mxrs-schema` →
`mxrs-forms` / `mxrs-settings` → `mxrs-model` → `mxrs-ir` → `mxrs-expr` → `mxrs-dsl` →
`mxrs-macros` (`project! {}` sugar) → `mxrs-writer` (fresh creation +
incremental re-sync) → `mxrs-project` (lossless imported snapshots and fresh
artifact rebuilds) → `mxrs-exporter` (`.mpr` → Cargo project) → `mxrs`
(application-facing facade crate).

**Compiler path** (an already-persisted model → Runtime shape → deployment):
`mxrs-model` → `mxrs-compiler-support` (shared normalization/identity) →
`mxrs-compiler-domain` (entities/security) →
`mxrs-compiler-flow` (microflow/nanoflow/code-action) →
`mxrs-compiler-widgets` (page metadata/web operations/navigation/settings/
artifact plus Gallery/Data Grid 2/Combo Box/Image and generic pluggable bundles,
shared modern list sources, and page/layout ES-module emission with native
structural/text rendering) →
`mxrs-javagen` (generated Java proxies) → `mxrs-materializers` (a versioned,
hash-verified React bundle plus stable `model.json`, embedded so normal builds
need no Node) → `mxrs-packager` (reproducible ustar deployment archives with
per-file SHA-256 integrity) → `mxrs-runtime` (transactional unit-of-work store,
fail-closed role/member authorization, and an action registry) →
`mxrs-runtime-http` (async static-app/action boundary that never trusts client
roles) → `mxrs-runtime-sqlite` (atomic durable snapshots) → `mxrs-oql`
(native OQL discovery, safe logical SQL projection, parameter extraction, and
query-risk analysis) → `mxrs-semantic` (deterministic artifact/reference index,
search, reverse references, and transitive impact) → `mxrs-scaffold`
(transactional new-project generation). Auth/session adapters, a native flow
interpreter, offline synchronization, and semantic mutation plans remain
follow-up layers.

`mxrs-cli` (binary `mxrs`) exposes import/build inspection, packaging, OQL,
semantic search/impact, project scaffolding, compatibility preflight, MDA
inspection, environment/cache diagnostics, Cargo-native upgrades, an isolated
PostgreSQL workspace, and offline Team Server repository status. Typed
domain import covers all attribute kinds plus documentation, length, date
localization, required/unique validation, and association owner/storage/docs.
Microflows and nanoflows can be authored and incrementally synchronized with
stable identities. Flow signatures support typed scalar, object, and list
parameters, documentation, required flags, and typed defaults. Calls in one
`project!` block check argument names and types at compile time; the writer
checks builder/IR calls against authored or existing native signatures before
writing. Parameter and type IDs survive synchronization. Imported graph shapes
not yet decompiled remain complete
in the snapshot; new server flows live in
`src/application/microflows/mod.rs`, while client flows live in
`src/presentation/nanoflows/mod.rs`. Project/module security is imported into
`src/domain/security/`, and modern navigation into
`src/presentation/navigation/`. Their writers validate role and target
invariants, preserve native fields outside the typed surface, and keep
existing identities. Unknown documents remain complete in the snapshot.
Task queues import as editable declarations under
`src/application/task_queues/`. `TaskQueueConfig` distinguishes legacy fixed
parallelism from modern expressions with per-node or cluster-wide scope.
Their metadata, queue/config identities, and folder placement survive sync;
unknown native fields remain in the imported snapshot and block standalone
export when they cannot be expressed completely. This is document authoring;
queue-backed flow execution remains part of the runtime backlog.
Pages have a typed front end too — see below.

Cargo-native domain code can use `#[derive(MxEntity)]` on structs and
`#[derive(MxEnumeration)]` on enums. Scalar field kinds are inferred from
`MxString`, `MxDecimal`, `MxDateTime`, and the other Mendix value types;
entity/attribute metadata uses `#[mxrs(...)]`. Enumeration references are
trait-checked rather than written as qualified-name strings, and enumeration
documents and localized captions retain stable identities across rebuilds.
Associations use `Reference<T>` or `ReferenceSet<T>` fields; their target and
cardinality are checked by Rust, while names and storage metadata can be
overridden with `#[mxrs(...)]`.

For example, flow parameters bind expressions that can be passed to another
flow or returned:

```rust,ignore
let project = mxrs::project! {
    "11.12.1",
    module Sales {
        microflow Echo {
            parameter message: string { documentation "Text to return"; }
            return message;
        }
        microflow Send {
            call Sales::Echo (message: mxrs::string("Hello")) -> response: string;
            return response;
        }
    }
};
```

Object and list declarations use `parameter order: object<Sales::Order>;`
and `parameter orders: list<Sales::Order>;`. All declared parameters require a
call argument; `required` and `default_value` are native parameter metadata,
not permission to omit arguments. Captured results declare their type after the name, such as
`-> order: object<Sales::Order>` or `-> orders: list<Sales::Order>`; the result
can be changed, iterated, passed to another call, or returned. The builder
counterpart is `flow.call_microflow_result::<MxObject<Order>>(target, "order",
arguments)`. Before writing, the target must return that type; void results,
unchecked captures, and duplicate variable names are rejected.

Cargo import reconstructs supported structured microflows and nanoflows as typed
builders: parameters, microflow calls, list creation, object create/change and
commit/delete, decisions, list/while loops, loop break/continue and final returns.
Branches and loop bodies can nest; variables remain local to their block.
Conditions currently accept boolean literals, parameters/results and direct
boolean attribute reads. Create/change assignments use typed attribute markers.
Expressions cover typed variables, direct attribute reads and canonical string,
boolean and numeric literals. Integer attribute reads can widen to Long through
`into_long()`; narrowing is never inferred. Calculated/autonumber writes,
associations, inherited or chained member paths and enum values remain outside
this projection. Attribute markers describe the imported schema; when changing
a domain attribute type, update the matching `TypedAttributeMarker::Value` too.
A rebuild without edits preserves the original bytes; edits with the same activity structure retain node identities,
layout and native metadata. Structural edits rebuild the activity graph while
retaining the flow identity. Branch returns, rescue paths, other activities and
unsupported expressions/options remain in the imported model. `mxrs portability` reports
these bodies as partial projections because native header and layout data still
come from the imported model; `--verify-round-trip` includes projected flows.
This reconstruction is part of Cargo import; standalone `export_project` keeps
its existing domain/document scope. Runtime interpretation remains pending.

Projects created before the four-layer layout can run
`mxrs upgrade --target .` for a read-only preview and add `--apply` to migrate
transactionally. Existing domain source stays untouched; the command adds the
composition root and missing layer aggregators so newer scaffolds have an
unambiguous destination.

Pages have a growing Cargo-native front end: `ModuleBuilder::page`
(`mxrs-dsl`) takes a nested-closure builder — the same "locked shape"
decision as microflows/entities, not a `#[derive(MxPage)]` struct, since a
widget tree has no natural 1:1 struct-field mapping. It covers layout
containers (`DivContainer`, lossless `LayoutGrid`), static text, buttons,
microflow/nanoflow-sourced DataViews, attribute-bound TextBox/CheckBox/
DatePicker/DropDown widgets, and name/class authoring for Data Grid 2,
Gallery, and ComboBox through `mxrs-pluggable`. All compile through
`mxrs-writer::page_compiler` onto `mxrs-forms`'s schema-driven Forms codec.
`mxrs import` detects the lossless structural subset, renders real builders
into `src/presentation/pages/mod.rs`, and wires them into the composition root
automatically.
Data-bound widgets, flow-calling buttons, empty official pluggable shells,
page metadata, object page parameters, and context-inherited DataViews import
to typed builders when every reference can be proven. Configured pluggable
widgets, action argument mappings, DataView footers, dynamic visibility/class
expressions, and unsupported native widgets remain snapshot-backed, never
guessed or dropped.
See `mxrs_ir::page` and `mxrs-exporter::page_export` for the explicit boundary.

## Cargo-native import

For a new application instead of an import:

```sh
mxrs new "Order Portal" --output order-portal --mxrs-workspace "$PWD"
cd order-portal
cargo run
```

During development of mxrs itself, point generated dependencies at this
workspace:

```sh
cargo run -p mxrs-cli -- import ExistingApp.mpr \
  --output existing-app \
  --mxrs-workspace "$PWD"
cargo install --path crates/app/mxrs-cli --bin cargo-mxrs
cd existing-app
cargo check
cargo test
cargo mxrs diff
cargo mxrs build --output build/ExistingApp.mpr
# Also writes the self-contained React application to build/web.

# Only when deliberately customizing/rebuilding the frontend:
cargo mxrs frontend-dev --output frontend
npm ci --prefix frontend
npm run build --prefix frontend

# Produce and independently verify a deterministic deployment archive:
mxrs package build/ExistingApp.mpr --web build/web --output build/ExistingApp.mxrs.tar
mxrs verify-package build/ExistingApp.mxrs.tar

# Inspect native OQL or project one safe query to logical SQL:
mxrs oql build/ExistingApp.mpr --dialect postgresql
mxrs translate-oql 'SELECT o/Number FROM Sales.Order o'

# Query the semantic graph:
mxrs refs build/ExistingApp.mpr Sales.Order
mxrs impact build/ExistingApp.mpr Sales.Order/Number
mxrs search build/ExistingApp.mpr orders
mxrs callers build/ExistingApp.mpr Sales.ACT_Save
mxrs callees build/ExistingApp.mpr Sales.ACT_Save
mxrs describe build/ExistingApp.mpr Sales.Order --json
mxrs tree build/ExistingApp.mpr Sales
mxrs lint build/ExistingApp.mpr --json
mxrs report build/ExistingApp.mpr

# Audit and prepare deployment inputs:
mxrs preflight build/ExistingApp.mpr --json
mxrs mda inspect build/ExistingApp.mda --json
mxrs cache warm build/ExistingApp.mpr
mxrs env . --environment production
mxrs doctor .

# Manage the partial, PostgreSQL-only Docker workspace:
mxrs db up build/ExistingApp.mpr
mxrs db status build/ExistingApp.mpr --json
mxrs db down build/ExistingApp.mpr

# Store only a pointer to a PAT and inspect an existing local repository:
mxrs team-server login --pat-file ~/.config/mendix/team-server.pat
mxrs team-server status .

# Discover implemented commands and exact usage:
mxrs --commands --json
mxrs help import
cargo mxrs --help
```

Installed/released builds omit `--mxrs-workspace` and generate the normal Git
dependency on the public `mxrs` facade.

## Development

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all -- --check
cargo run -p xtask -- oracle-diff xtask/fixtures/minimal
cargo run -p xtask -- oracle-diff xtask/fixtures/with_page
cargo run -p xtask -- capability-matrix
npm ci --prefix crates/compiler/mxrs-materializers/frontend
npm run build --prefix crates/compiler/mxrs-materializers/frontend
```

`oracle-diff` is the correctness ceiling for anything touching the
BSON/writer/model path: it runs the real `mxrb` (Ruby) install as a
behavioral oracle and requires a byte-identical round trip on both fixtures.
`MXRB_HOME` overrides the default oracle location (`../mxrb`, beside this
workspace).

`capability-matrix` reads MXRB's own command inventory and classifies every
row as verified, partial, or missing. Its identity-based CI ratchet records 76
commands: 0 verified, 58 partial, and 18 missing. Earlier claims of four
verified commands lacked command-specific differential evidence and were
corrected. "Partial" never means
command parity; each row stays incomplete until its full observable contract
has an executable oracle.

`lint` and `report` cover explicit reference fields, recursive call components
and module dependencies, not all Studio Pro consistency checks. Their JSON
reports include the analysis boundary. Imported layouts and unsupported page
fields remain snapshot-backed; a typed page is emitted only if recompiling it
preserves the original native fields and references. New projects author a
local `Main.ApplicationLayout` and navigation home rather than referencing an
uninstalled Atlas module.

Measure coverage with `bash xtask/coverage.sh target/coverage.json`, then check it
with `cargo run -p xtask -- coverage-gate target/coverage.json`. The driver creates
fresh outer and nested Cargo directories, includes every instrumented ELF
(including generated applications), and retains profiles, source/object hashes
and toolchain details. It does not delete old evidence. For limited `/tmp` space,
use `MXRS_COVERAGE_TMPDIR="$PWD/target" bash xtask/coverage.sh`.
The gate prints the exact counts for the current source revision; record them
with that commit instead of carrying percentages into a later revision.
Development floors remain 82/82/81/66%.
`--require-complete` requires exact covered/count equality for all four metrics.
Release tags and manual readiness CI runs enforce that strict gate. Coverage
must be measured from clean instrumented artifacts, with no source exclusions.
`cargo llvm-cov clean --workspace` alone is insufficient across toolchains:
old-layout binaries can survive and contribute obsolete coverage maps.
Passing development floors is not 100% coverage or proof of Studio Pro parity.

See [verification and remaining parity work](docs/verification.md) for exact
readiness commands, the unversioned-model acceptance procedure, and the limits of
the available evidence.
