# Design command contract

`mxrs design init [--target DIR] [--dry-run] [--json]`,
`mxrs design scan FILE.mpr [--json]` and
`mxrs design migrate FILE.mpr LITERAL TOKEN [--apply] [--json]` port native
`mxrb design` over the stylesheet assets living next to an `.mpr` — the same
`ASSET_DIRECTORIES` the compare snapshot inventories (`theme`, `theme-cache`,
`themesource`, `resources`, `widgets`, `javasource`, `javascriptsource`,
`userlib`, `vendorlib`) — so they behave identically for Studio Pro exports
and Cargo-built models. The oracle
(`cargo run -p xtask -- command-oracle design <mxrs-binary>`) compares scan
and migrate over the same theme fixture (human output modulo the
`[mxrb]`/`[mxrs]` prefix, JSON parsed structurally), asserts `--apply` leaves
byte-identical stylesheets behind, pairs `init` between a native project and
a Cargo project, and shares refusals for the same invalid inputs.

| Action | Behavior |
| --- | --- |
| `init` | Materializes the theme kit (`theme/web/{custom-variables,main,exclusion-variables}.scss`, `theme/web/settings.json`, `theme-cache/web/theme.compiled.css`). Files already present are left untouched (native `ensure_file`), so re-running never clobbers an edited theme. |
| `scan` | Inventories every `*.css`/`*.scss` under the asset directories, sorted by full path: CSS custom properties (`--name: value;`) and SCSS variables (`$name: value;`), with `_theme-<name>.scss` theme attribution, `design-properties.json` catalogs under `themesource/` (unparsable catalogs stay `null`), unresolved `var(--…)` references, and literal-color tokens. `//`-commented lines are skipped. |
| `migrate` | Previews literal→token replacements across the same stylesheets (symlinks excluded); prints per-file occurrence counts. `--apply` re-verifies each asset's SHA-256 digest against the preview, stages every replacement next to its target, then renames into place — a stale plan refuses to double-apply and a failed rename restores the original bytes. |

Both `scan` and `migrate` open the model store first, so a missing or
corrupt `.mpr` is refused rather than treated as an empty theme.

## What is not ported

The `design_system` Ruby DSL policy block — `forbid_literal_colors`
enforcement and the `DesignMaterializer` that serializes a typed
design-system contract into `theme/web/_mxrb-design-system.scss` plus a
generated `design-properties.json` — has no MXRS DSL surface yet and is not
pretended here. `design init` also does not write the native
`design_system` Ruby policy file, which would be meaningless in a Cargo
project.
