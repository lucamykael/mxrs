# `dump-unit`

`mxrs dump-unit FILE.mpr UNIT_ID` prints the unit's stored identity, container,
containment name, native type, contents hash, and a hexadecimal/ASCII dump of
its original bytes. Reading a dump never reserializes or modifies the unit.

The command supports inline MPR v1 contents and externally stored MPR v2
contents, including nested `mprcontents` paths. UUID lookup accepts uppercase
letters and the compact hexadecimal form without hyphens; output identities
are canonical lowercase UUIDs.

## Output and errors

Successful output matches the native MXRB command byte for byte:

- Headers retain the stored `UnitID`, `ContainerID`, `ContainmentName` and
  `ContentsHash`. The type is read from the same bytes that are dumped.
- A missing type or contents hash prints an empty header value.
- Hex rows contain up to 16 bytes. Offsets use at least four hexadecimal
  digits and expand for larger units. ASCII bytes 32–126 print literally;
  other bytes print as `.`. The last row is padded to the native width.
- Absent contents print `  (empty)`. A present, zero-byte payload prints no
  rows after `Contents (hex)   :`. This distinction also applies to empty
  versus missing external unit files.
- Unknown units, invalid UUIDs, inaccessible/invalid projects and corrupt BSON
  fail with a diagnostic. An unknown but valid native type can still be dumped.

MXRS validates the contents before printing. For corrupt BSON, MXRB may have
already printed identity headers before it fails; MXRS emits only the error
on stderr. Error status/channel are verified without copying Ruby stack traces.

`--no-progress` is accepted for compatibility. Help is available with `--help`,
`-h`, or `mxrs help dump-unit`. MXRS rejects unknown/repeated flags and extra
positional arguments that the native handler may ignore. Prefix dash-leading
file paths with `./`.

## Reproduce verification

```sh
cargo build -p mxrs-cli
cargo run -p xtask -- command-oracle dump-unit target/debug/mxrs
```

The developer-only oracle uses the read-only sibling MXRB checkout (override
with `MXRB_HOME`). Its 44 checks execute both actual CLIs on disposable native
v1/v2 projects. They compare complete successful stdout, including a payload
containing all byte values and offsets beyond `ffff`, plus empty/absent
contents, missing metadata, UUID variants, help and errors. File hashes and
file inventories remain unchanged. Temporary fixtures are removed on success
or failure; no private corpus or network access is required.

Rust CLI regression tests cover metadata and byte recovery, large offsets,
ASCII boundaries, empty/absent contents and failures without partial output.
`Verified` refers to this raw inspection contract, not to model editing or
Mendix compiler/runtime compatibility.
