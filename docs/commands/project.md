# Project inspection contract

`mxrs project inspect [DIR] [--json] [--no-progress]` ports MXRB's workspace
inventory to Cargo-native sources. DIR defaults to the current directory. The
command reports source and filesystem facts without executing project code or
opening/validating the discovered MPR databases.

## Explicit language mapping

| Fact | MXRB | MXRS |
| --- | --- | --- |
| Root | Lexically expanded absolute directory | Same; symlinks are not resolved |
| Project definition exists | `project_file`: `project.rb` exists | `manifest`: `Cargo.toml` exists |
| Declared version | `mendix_version` in `project.rb` | Generated `mxrs.toml` version, then Rust application attribute |
| Modules | `modules/*/module.rb` parent names | `src/domain/modules/*/mod.rs` parent names |
| MPR inventory | Sorted root `*.mpr` paths | Same, plus sorted `build/*.mpr` paths |
| Registered scaffolds | Sorted keys in `.mxrb/scaffolds.json` | Sorted keys in `.mxrs/scaffolds.json` |

MXRS additionally reports `domain_module` (`src/domain/mod.rs` exists) and
`layout` (`layered`, `pre-layered` or `incomplete`, recognizing generated source
layouts). Module directory names are reported as stored; Cargo names are normally
snake_case. Hidden module directories and dot-prefixed MPRs are excluded, like
native glob enumeration. This is inventory, not semantic validation of modules or Cargo.

The Rust application attribute is parsed as syntax, allowing whitespace,
multiline/reordered arguments and raw string literals. Comments and example
strings cannot supply a version. Invalid Rust syntax, nonliteral versions and
multiple declared application versions fail explicitly. The generated manifest
has precedence when its `mendix_version` declaration is present.

An absent/empty directory produces an empty inventory. An existing unreadable
inventory path or invalid registry fails; MXRS does not hide an unreadable
`build` or modules directory as an empty list. Registry entries also receive
MXRS's existing path/shape checks. These checks are stricter than native glob
and registry-key enumeration.

JSON contains every reported fact. Human output contains one `key: value` line
per fact; lists use comma separators, missing versions print blank. Both help
forms and `--no-progress` work. Unknown/repeated flags, unknown actions and extra
arguments fail. Diagnostic text need not reproduce Ruby stack traces.

## Repeat verification

```sh
cargo build -p mxrs-cli -p xtask
target/debug/xtask command-oracle project target/debug/mxrs
```

The oracle builds paired native/Cargo workspace representations in disposable
directories and executes both actual CLIs. It compares **all** native inventory
facts using the table above, checks Cargo-specific facts separately, and verifies
that every JSON field appears in the human report. Cases include absent/empty
workspaces, default/explicit/lexical paths, version sources, multiline Rust,
module directories, registry ordering, build outputs, errors and arguments.
Hash/inventory checks prove no source execution or mutation occurred.

Verified applies to this workspace inventory with its explicit language mapping;
it does not establish source-generator, compiler or runtime parity.
