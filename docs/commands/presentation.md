# Presentation initialization contract

`mxrs presentation init MODULE [--target DIR] [--dry-run] [--json]`
initializes presentation in an existing Cargo project and existing module,
matching native `mxrb presentation init`. It also accepts `--no-progress`,
`--help` and `-h`. The target defaults to the current directory.

| Native workspace | Cargo workspace |
| --- | --- |
| `modules/Sales/presentation/presentation.rb` | `src/presentation/modules/sales/mod.rs` and `layouts/{mod.rs,application_layout.rs}` |
| `pages/.keep` | `pages/{.keep,mod.rs}` |
| `snippets/.keep` | `snippets/{.keep,mod.rs}` |
| `client_actions/.keep` | `nanoflows/{.keep,mod.rs}` |
| Module's Ruby presentation loader | Rust presentation module registry |
| `.mxrb/scaffolds.json` | `.mxrs/scaffolds.json` |

Rust module/family registries connect the generated declarations to the build.
Snippets get an empty family ready for authored declarations. This does not add
a standalone snippet generator or full frontend runtime support.

## Native application layout

The generated Rust is editable and compiles immediately:

```rust
module.layout("ApplicationLayout", |layout| {
    layout.class("mxrb-application-shell");
    layout.application_shell("ApplicationLayout", Some("Responsive"));
});
```

The typed composition contains the native scroll container, a 264-pixel
navigation sidebar with its toggle button, a 72-pixel header with the title,
and the `Main` content placeholder. `None` disables the sidebar; the title and
navigation profile are editable. Native CSS class names are preserved because
the existing application styles refer to them. Writer code uses checked Forms
nodes; generated Rust contains no BSON or storage IDs.

MXRS explicitly declares the `Main` layout parameter used by generated page
calls. Native MXRB's layout helper derives it from the placeholder. Verification
checks that additional declaration and compares the entire remaining layout
after strict native Forms decoding/encoding. Generated IDs and null-versus-
absent fields are normalized; other fields, defaults and widget order must
agree, including a customized title with navigation disabled.

This composition is an authoring surface. Importing an arbitrary native scroll
container still follows the existing lossless preservation boundary; it does
not imply general typed support for every navigation/container widget.

## Files, output and errors

The preview prints the exact paths that applying will create/update and leaves
the workspace unchanged. JSON has native `kind`, `name`, `dry_run`, `files` and
`updated` fields. Human output keeps the native prefixes and ordering; its final
build hint is `cargo mxrs build`.

An existing presentation aggregator plus all three keep files counts as
initialized and is refused. An incomplete setup recovers missing directories
and keeps existing source files. Missing projects/modules, invalid names,
unknown/extra/repeated flags and conflicting files fail before publication.
Rust additionally refuses an orphan layout source without its family registry.
The scaffold transaction registers files created by that execution, as native
MXRB does; repairs do not claim ownership of existing files.

## Repeat verification

```sh
cargo build -p mxrs-cli -p xtask
target/debug/xtask command-oracle presentation target/debug/mxrs
```

The oracle executes both CLIs on disposable paired workspaces, compares
preview/apply receipts against actual file changes, checks both human/JSON
output, duplicate/error behavior, registry entries and incomplete-directory
recovery, compiles the Ruby and Rust projects and compares their native layouts.
Rust regressions also cover both navigation modes, duplicate placeholders,
rejection of shells in ordinary pages and compiled scaffold output.
