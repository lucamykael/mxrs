# `mxrs preflight`

```sh
mxrs preflight <file.mpr> [--json]
```

Says, read-only, what a model holds that mxrs's native compilers and
runtime do not take, in mxrb's report shape. It exits 1 when a finding is
an error.

- **version**: mxrs compiles Mendix 11 models. Another major version is one
  error, and nothing else of the model is audited.
- **storage**: what `mxrs validate` finds.
- **widget**, **custom_widget**, **page**: what the page and layout compiler
  does not draw.
- **flow**, **nanoflow**: activities and code actions the flow compiler and
  the nanoflow runtime do not take, and their warnings.

Identical findings are one, with a `count`, ordered by severity, category,
location and type. The text output is mxrb's:

```text
Project : /path/App.mpr
Mendix  : 11.12.1
Units   : 42
[ERROR] widget Forms$Gallery at Sales.Overview: Forms$Gallery is not compiled by the native web renderer
[mxrs] 1 error(s), 0 warning(s)
```

`--json` prints `{path, mendix_version, compatible, stats, findings}`; the
stats add mxrs's `flows` and `code_actions` counts to mxrb's.

Which constructs are unsupported is each tool's own coverage, so mxrb and
mxrs can disagree on a model one of them draws more of;
`xtask command-oracle preflight` compares the two line for line on a model
both take whole.
