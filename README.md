# mxrs

A ground-up Rust implementation of Mendix Studio Pro `.mpr` project reading,
writing, compiling and packaging — no Ruby, no mxcli, single static binary.

Sibling project to [mxrb](https://github.com/lucamykael/mxrb) (Ruby), which
remains the reference implementation and behavioral oracle during
development, but is not a runtime dependency of anything shipped here.

The full phased implementation plan (crate layout, dependency DAG, milestone
done-definitions, DSL/macro design, testing strategy) lives in this
project's `ai-memory` as `decisions/mxrs-rust-rewrite-plan.md`.

## Status

31 crates + a dev-only `xtask` harness, ~50k lines of Rust, over 500 tests
passing. The primary direction is now Cargo-native: Rust is the editable
source of truth and `.mpr` is an import/export build artifact.

```text
existing .mpr -> mxrs import -> Cargo project -> cargo check/test
                                          \-> cargo run -> new .mpr
```

An import writes typed domain source under `src/domain/`, stable identity
bindings and public marker types under `src/infrastructure/`, and a complete generated unit snapshot
under `model/imported/`. Mendix filesystem resources are copied into the
editable `assets/` tree and materialized next to each built `.mpr`. The
snapshot makes concepts without a friendly Rust
representation lossless; rebuilding does not read or patch the original
`.mpr`. Typed coverage can therefore replace opaque snapshot content
incrementally without blocking a correct build.

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
fail-closed role/member authorization, and an action registry) → `mxrs-oql`
(native OQL discovery, safe logical SQL projection, parameter extraction, and
query-risk analysis) → `mxrs-semantic` (deterministic artifact/reference index,
search, reverse references, and transitive impact) → `mxrs-scaffold`
(transactional new-project generation). HTTP/storage adapters and semantic
mutation plans remain follow-up layers.

`mxrs-cli` (binary `mxrs`) exposes `import`, `validate`, `compare`, `inspect`,
`units`, `dump-unit`, `sql`, `modules`, `export`, and `javagen` today. Typed
domain import covers all attribute kinds plus documentation, length, date
localization, required/unique validation, and association owner/storage/docs.
Microflows and nanoflows can be authored and incrementally synchronized with
stable identities; imported graph shapes not yet decompiled remain complete
in the snapshot and are listed in `src/domain/flows/mod.rs`. Project/module
security and modern navigation profiles are imported into typed
`src/domain/{security,navigation}/` modules. Their writers validate role and
target invariants, preserve native fields outside the typed surface, and keep
existing identities. Unknown documents remain complete in the snapshot.
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
into `src/domain/pages/mod.rs`, and wires them into `build()` automatically.
Data-bound widgets, flow-calling buttons, and pluggable widget identities
remain opaque on import until generated markers and richer decode metadata
land; they are preserved in the imported snapshot, never guessed or dropped.
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
npm ci --prefix crates/compiler/mxrs-materializers/frontend
npm run build --prefix crates/compiler/mxrs-materializers/frontend
```

`oracle-diff` is the correctness ceiling for anything touching the
BSON/writer/model path: it runs the real `mxrb` (Ruby) install as a
behavioral oracle and requires a byte-identical round trip on both fixtures.
`MXRB_HOME` overrides the default oracle location
(`/home/mykael/Personal_Projects/mxrb`).
