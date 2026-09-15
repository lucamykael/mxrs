# `modules`

`mxrs modules FILE.mpr` reads a project and prints each module's name and
entity, page and microflow counts, in the order returned by the model store.

```text
"Sales": entities=2 pages=3 microflows=4
```

Names use JSON string escaping. An unnamed module prints `null`; it remains
present in the report. Empty projects print no records. Both inline MPR v1
and MPR v2 with external unit contents are supported. The command is read-only.

## Options

- `--json`: an array of `{name, entities, pages, microflows}` records; an empty
  project produces `[]`.
- `--names`: the previous MXRS output, one named module per line, sorted by
  name. Unnamed modules are omitted only in this compatibility mode.
- `--no-progress`: accepted for MXRB command compatibility; this read-only
  command has no progress display.
- `--help`, `-h`, or `mxrs help modules`: show usage.

`--json` and `--names` are mutually exclusive. Unknown/repeated options and
extra positional arguments fail with a diagnostic. Paths beginning with a
dash must be prefixed with `./`.

## Verified MXRB contract

The reference is the `modules` handler in `mxrb/bin/mxrb` and
`Mxrb::Model::Module#inspect`. The verified result is the ordered sequence of
module names and all three counts:

- Entities come from the module's domain model; a missing domain model counts
  as zero.
- Pages include both `Forms$Page` and `Pages$Page` documents.
- Microflows include `Microflows$Microflow` documents. Nanoflows, rules and
  other document kinds do not contribute to that count.
- Documents inside nested folders count toward their owning module.
- Missing arguments, missing/unreadable projects and invalid databases fail.
  Help succeeds. Successful reads and failures leave project files unchanged.

The default presentation is native to each CLI: MXRB uses Ruby inspection
syntax, while MXRS uses the concise records above. Verification normalizes
that syntax, without sorting away order differences or dropping fields.
Diagnostics are compared by failure status and output channel rather than by
Ruby stack-trace wording. MXRS additionally rejects trailing arguments and
unknown options that the native handler silently ignores; JSON and names-only
output are MXRS extensions.

## Reproduce verification

```sh
cargo build -p mxrs-cli
cargo run -p xtask -- command-oracle modules target/debug/mxrs
```

The developer-only oracle uses the read-only sibling MXRB checkout (override
with `MXRB_HOME`). It generates disposable native fixtures and executes both
real CLIs. Its 22 checks cover empty/v1/v2 projects, nested folders, both page
types, excluded flow/document kinds, missing and escaped Unicode names,
storage order, names/JSON modes, help, errors and unchanged file hashes.
Fixtures are removed on completion or failure. No private corpus or network
access is needed. Rust CLI regression tests also run in the workspace suite.

`Verified` for this command describes this read-only listing contract. It
does not establish parity for model editing, the Mendix compiler or runtime.
