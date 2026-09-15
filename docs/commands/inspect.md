# Native inspect contract (`mxrs units`)

`mxrs units FILE.mpr [--no-progress]` is the counterpart of
`mxrb inspect FILE.mpr`. The existing `mxrs inspect` command is a separate
structural snapshot interface.

Successful output matches MXRB byte for byte:

- Project name: root unit's `Name`, then lowercase `name`, then the MPR filename
  without `.mpr`; no root means a blank project name.
- Mendix version and alphabetically sorted SQLite table names.
- Sorted, distinct unit type names, excluding absent types.
- All units in storage order, with UUID, type, container and containment name.

No filtering by supported model families occurs. Unknown but valid document
types remain visible. Each unit is decoded once; corrupt BSON or storage errors
fail before any report is printed. MXRB may print a header before such errors.
Missing names/types print empty fields. v1 inline and v2 external contents work.

Both help flags and `--no-progress` work. MXRS rejects extra arguments,
unknown/repeated flags and invalid inputs with nonzero status and stderr.
Ruby stack traces are not part of the contract. Inspection never mutates or
compiles the project.

## Repeat verification

```sh
cargo build -p mxrs-cli -p xtask
target/debug/xtask command-oracle inspect target/debug/mxrs
```

The oracle runs both CLIs against disposable native v1/v2 projects, comparing
successful stdout without sorting or normalizing it. Cases include root name
variants, filename fallback, empty stores, unknown/missing types, corrupted BSON,
help and bad arguments/paths. File inventories and SHA-256 hashes must remain
unchanged. A Rust regression test ensures corrupt BSON cannot produce a
successful or partial inventory.
