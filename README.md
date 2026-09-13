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

25 crates + a dev-only `xtask` harness, ~50k lines of Rust, 490 tests
passing. The primary direction is now Cargo-native: Rust is the editable
source of truth and `.mpr` is an import/export build artifact.

```text
existing .mpr -> mxrs import -> Cargo project -> cargo check/test
                                          \-> cargo run -> new .mpr
```

An import writes typed domain source under `src/domain/`, stable identity
bindings under `src/generated/ids.rs`, and a complete generated unit snapshot
under `model/imported/`. Mendix filesystem resources are copied into the
editable `assets/` tree and materialized next to each built `.mpr`. The
snapshot makes concepts without a friendly Rust
representation lossless; rebuilding does not read or patch the original
`.mpr`. Typed coverage can therefore replace opaque snapshot content
incrementally without blocking a correct build.

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
`mxrs-javagen` (generated Java proxies). Materializers/packaging
(`mxrs-materializers`, `mxrs-packager`) and the runtime/OQL/semantic/scaffold
subsystems are not started yet.

`mxrs-cli` (binary `mxrs`) exposes `import`, `validate`, `compare`, `inspect`,
`units`, `dump-unit`, `sql`, `modules`, `export`, and `javagen` today. Typed
domain import covers all attribute kinds plus documentation, length, date
localization, required/unique validation, and association owner/storage/docs.
Flows, security, navigation, and unknown documents remain complete in the
generated snapshot while their ergonomic Rust front ends are built. Pages
now have a first typed front end too — see below.

Cargo-native domain code can use `#[derive(MxEntity)]` on structs and
`#[derive(MxEnumeration)]` on enums. Scalar field kinds are inferred from
`MxString`, `MxDecimal`, `MxDateTime`, and the other Mendix value types;
entity/attribute metadata uses `#[mxrs(...)]`. Enumeration references are
trait-checked rather than written as qualified-name strings, and enumeration
documents and localized captions retain stable identities across rebuilds.
Associations use `Reference<T>` or `ReferenceSet<T>` fields; their target and
cardinality are checked by Rust, while names and storage metadata can be
overridden with `#[mxrs(...)]`.

Pages have a first, narrow Cargo-native front end: `ModuleBuilder::page`
(`mxrs-dsl`) takes a nested-closure builder — the same "locked shape"
decision as microflows/entities, not a `#[derive(MxPage)]` struct, since a
widget tree has no natural 1:1 struct-field mapping. Only native/structural
widgets are covered — layout containers (`DivContainer`, `LayoutGrid`),
static text, and buttons (no-op or close-page only) — compiled by
`mxrs-writer::page_compiler` onto `mxrs-forms`'s schema-driven Forms
metamodel codec. Pluggable widgets (Data Grid 2/Gallery/ComboBox/generic
bundle), native data-bound widgets (attribute-bound TextBox/CheckBox/...),
button actions that call a microflow/nanoflow, and flows/security/navigation
on pages are all explicitly deferred — see `mxrs_ir::page`'s doc comment.
`mxrs import` detects pages built entirely from this vocabulary and renders
them for review into `src/domain/pages.rs`; that file is intentionally *not*
wired into the live build (see `mxrs-exporter::page_export`'s doc comment
for why) — adopting a page means copying its generated body into your own
`module.page(...)` call.

## Cargo-native import

During development of mxrs itself, point generated dependencies at this
workspace:

```sh
cargo run -p mxrs-cli -- import ExistingApp.mpr \
  --output existing-app \
  --mxrs-workspace "$PWD"
cargo install --path crates/mxrs-cli --bin cargo-mxrs
cd existing-app
cargo check
cargo test
cargo mxrs diff
cargo mxrs build --output build/ExistingApp.mpr
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
```

`oracle-diff` is the correctness ceiling for anything touching the
BSON/writer/model path: it runs the real `mxrb` (Ruby) install as a
behavioral oracle and requires a byte-identical round trip on both fixtures.
`MXRB_HOME` overrides the default oracle location
(`/home/mykael/Personal_Projects/mxrb`).
