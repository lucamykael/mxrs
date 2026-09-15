# Native MPR diff contract

`mxrs diff LEFT.mpr RIGHT.mpr [--json] [--no-progress]` implements native
`mxrb diff`, using the [comparison snapshot and rules](compare.md).

The default output is one tab-separated record per ordered change:

```text
changed	modules.Main.entities.Record.documentation	"old"	=>	"new"
```

Operations are `added`, `removed` or `changed`. Paths join names and indices
with dots, as in native diff. Values use compact, sorted JSON instead of Ruby
`inspect`, with `nil` for absence. Binary/nonfinite BSON uses extended JSON;
floating notation can differ without changing the numeric value.

Identical inputs produce empty stdout and exit 0. Differences exit 1. JSON
output is the same `{identical, changes}` extension as `mxrs compare`, with
typed path segments and capitalized operation names. Errors, argument checks,
read-only guarantees, asset restrictions and duplicate-flow sorting follow the
comparison contract. `cargo mxrs diff` is a separate source-workspace command
that builds the Cargo project before comparison.

## Repeat verification

```sh
cargo build -p mxrs-cli -p xtask
target/debug/xtask command-oracle diff target/debug/mxrs
```

This runs the full shared comparison corpus through the actual native and Rust
`diff` handlers, validating their distinct tabular output and empty identical
result, status, complete ordered changes, errors and read-only behavior.
