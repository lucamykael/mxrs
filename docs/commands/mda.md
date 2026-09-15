# MDA command contract

```text
mxrs mda inspect FILE.mda [--json] [--no-progress]
mxrs mda compare LEFT.mda RIGHT.mda [--no-progress]
```

These commands port `mxrb mda inspect/compare`, operating on existing ZIP
archives without extracting, modifying, compiling or deploying them.

## Inspection

Inspection requires `model/metadata.json`. Human output includes runtime,
project, file count, sorted archive roots and the SHA-256 of the original
archive. Absent/null runtime and project names print blank fields. JSON reports
the absolute path, archive SHA-256, roots, file count and complete metadata.
Explicit directory entries contribute roots but do not count as files.

MXRS requires metadata to be an object with optional string/null
`RuntimeVersion` and `ProjectName`; additional metadata fields are preserved.
This is stricter than MXRB accepting some other JSON shapes before rendering.

## Comparison

Comparison emits sorted `added`, `removed` and `changed` paths, then
`[mxrs] N difference(s)`. Both identical and different valid archives exit zero,
as MXRB does. File bytes determine equality; ZIP entry order and archive-level
hash differences alone do not imply content differences. Directories are omitted
from comparison. Metadata is a compared file like any other.

Successful inspection output matches MXRB exactly; JSON key order is irrelevant.
Comparison normalizes only `[mxrb]` to `[mxrs]`.

## Invalid archives and arguments

Missing metadata, corrupt ZIP/JSON and traversal/dot paths fail with nonzero
status and stderr before output. MXRS also rejects leading-slash paths, duplicate
normalized entries and repeated path separators; it never extracts entries.
A single trailing separator is allowed for directory entries. Backslashes are
normalized for inventory names. These stricter checks avoid ambiguous archives.

Both help forms and `--no-progress` work. Unknown/repeated flags and extras are
rejected; `--json` applies only to inspection. Ruby stack traces are not copied.
No guarantee of successful Mendix deployment follows from a valid inventory.

## Repeat verification

```sh
cargo build -p mxrs-cli -p xtask
target/debug/xtask command-oracle mda target/debug/mxrs
```

The oracle builds disposable ZIPs independently of the Rust reader and invokes
both CLIs. It covers metadata, directory/empty entries, Unicode/binary files,
reordered archives, added/removed/changed content, invalid metadata/ZIP/paths,
help and arguments. Stricter Rust-only archive refusals are explicit cases.
All source hashes and file inventories must remain unchanged.
