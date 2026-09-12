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

21 crates + a dev-only `xtask` harness, ~34k lines of Rust, 412 tests
passing. Two independent pipelines share only the compiled model as input:

**Writer path** (DSL definition → `.mpr`, what Studio Pro opens):
`mxrs-bson` → `mxrs-mpr` / `mxrs-fragment-store` / `mxrs-schema` →
`mxrs-forms` / `mxrs-settings` → `mxrs-model` → `mxrs-ir` → `mxrs-expr` → `mxrs-dsl` →
`mxrs-macros` (`project! {}` sugar) → `mxrs-writer` (fresh creation +
incremental re-sync) → `mxrs-exporter` (`.mpr` → editable Rust source, the
other half of the round trip).

**Compiler path** (an already-persisted model → Runtime shape → deployment):
`mxrs-model` → `mxrs-compiler-support` (shared normalization/identity) →
`mxrs-compiler-domain` (entities/security) →
`mxrs-compiler-flow` (microflow/nanoflow/code-action) →
`mxrs-compiler-widgets` (page metadata/web operations/navigation/settings/
artifact plus Gallery/Data Grid 2/Combo Box/Image and generic pluggable bundles,
with shared modern list sources; page-module integration is in progress) →
`mxrs-javagen` (generated Java proxies). Materializers/packaging
(`mxrs-materializers`, `mxrs-packager`) and the runtime/OQL/semantic/scaffold
subsystems are not started yet.

`mxrs-cli` (binary `mxrs`) exposes `validate`, `compare`, `inspect`, `units`,
`dump-unit`, `sql`, `modules`, `export`, `javagen` today — a narrow slice of
`mxrb`'s ~80-command surface, growing as the corresponding engine crates land.

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
