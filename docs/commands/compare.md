# Structural comparison contract

`mxrs compare LEFT.mpr RIGHT.mpr [--json] [--no-progress]` compares the
same structural snapshot as native `mxrb compare`:

- Project/storage versions and the complete non-root unit type/name inventory.
- Security, navigation and hashes of regular design asset files.
- Modules, entities, attributes, generalization, access rules, associations and
  delete behavior; module roles; pages, widget options/events and menus.
- Microflows/nanoflows, return types, permissions, parameters and normalized
  activity/edge graphs. Attribute native type/value properties are retained.

This is the native comparison scope, not a comparison of every stored BSON
property or a proof of runtime equivalence. Unknown unit types contribute to
the unit inventory; their complete document bodies are outside this snapshot.

Named collections compare by unique names. Equal content under a different
name produces a rename at the old name's path. Other arrays compare by index.
Numeric integer/double values compare by value, without rounding large
integers. Flow traversal replaces activity IDs and removes editor layout;
parameters, access-rule IDs and module-role IDs remain visible as in MXRB.

Design assets cover `theme`, `theme-cache`, `themesource`, `resources`,
`widgets`, `javasource`, `javascriptsource`, `userlib` and `vendorlib` beside
each MPR. Hidden entries and nested symlinks are ignored. Distinct filenames,
including a literal backslash on Unix, keep distinct keys.

## Output and deliberate differences

Identical projects print `[mxrs] OK` and exit 0. Differences print one
`[mxrs] diff: PATH: BEFORE != AFTER` per ordered change and exit 1, with no
summary line. Indexed path segments render as `[0]`; names remain names.
Absent values render as `nil`. Other values use compact JSON with sorted
object keys instead of Ruby `inspect`; BSON binary/nonfinite values use
extended JSON. Floating-point notation can differ while preserving the value.

`--json` is an MXRS extension: `{identical, changes}`. Each change has
`operation` (`Added`, `Removed`, `Changed`), `path`, `before` and `after`.
Path arrays preserve numeric array indices and string names, including numeric
names. Absence is JSON null. Both output modes preserve change ordering.

Two restrictions and one stability improvement are intentional:

- Asset traversal/read errors and non-UTF-8 filenames fail explicitly. An
  expected asset directory occupied by a regular file also fails.
- Malformed complex attribute defaults fail instead of relying on Ruby's
  container-to-string formatting. Scalar native defaults retain that formatting.
- Flows with duplicate names use normalized content as a sorting tie-breaker.
  MXRB instead preserves SQLite order among equal names. Reordering those rows
  alone is identical in MXRS; a body edit still reports a change. The oracle
  executes and checks this divergence explicitly.

Invalid MPRs, corrupt documents and invalid/extra/repeated options fail with
stderr and no report. Ruby stack traces are not an output contract. Both help
flags work. Inputs and adjacent assets are never modified.

## Repeat verification

```sh
cargo build -p mxrs-cli -p xtask
target/debug/xtask command-oracle compare target/debug/mxrs
```

The oracle executes both real CLIs on disposable native v1/v2 projects. It
checks complete snapshots and ordered changes, identical/changed status,
human/JSON output, reverse comparisons, renames, numeric defaults, layout and
row reordering, unknown units, asset paths, errors and read-only hashes. Human
value mapping allows only the documented JSON and floating notation changes.
Rust regressions also exercise numeric boundaries, binary values and typed
paths that distinguish indices from numeric names.
