# `mxrs frontend migrate`

```sh
mxrs frontend migrate <file.mpr> [--apply] [--json]
```

Previews the frontend migrations a model needs after its installed widget
packages, its theme or its Mendix version moved on, and with `--apply`
writes them. Only Mendix 10 and 11 models are migrated.

```text
Version           : 11.12.1
Changes           : 1
Widgets           : 1
Layout rows       : 3
Design properties : 2
Safe              : true
Applied           : false
```

`Changes` counts the units that would change; the next three lines count
each migration among them. `--json` prints the same as
`{version, changes, widgets, layout_rows, design_properties, issues, safe,
applied}`, each issue `{unit_id, path, kind, message}`.

## What it migrates

- **Pluggable widgets.** Each `CustomWidgets$CustomWidget` is rebound to
  the schema its package in the project's `widgets/*.mpk` declares. A
  configured value moves to the property of the same key and keeps its
  identities; only the fields its value kind uses are carried over, and a
  carried-over data source or action gains the fields the current model
  requires (`OutputMappings`, a snippet parameter's `SubKey`). Stored
  properties keep their order and the package's new ones follow. A `Boolean`
  that only says `true` or `false` becomes that `Expression`. Nested object
  lists are rebound object by object. A caption left at its default while
  the switch it labels is off is stored empty, and with the audited Data
  Grid 2 package installed, the texts the grid's own settings hide are too.
- **Layout-grid rows.** Desktop column weights must be positive and total
  twelve. A proportional (negative) weight takes its share of what the
  fixed columns leave, by largest remainder, so `[-2, -1, -2]` becomes
  `[5, 2, 5]`. Tablet and phone weights keep `-1`, which there means
  "inherit".
- **Design properties.** A property or option the theme renamed
  (`oldNames` in `themesource/*/{web,native}/design-properties.json`) takes
  its new name, and a legacy spacing option becomes its side of the
  compound `Spacing` property.

## Fail-closed

Whatever cannot be migrated losslessly is an issue, printed after the
report as `[KIND] <unit><path>: <message>`. A plan with any issue is not
safe: the command exits 1 and `--apply` writes nothing.

| Kind | Meaning |
| --- | --- |
| `unsupported_version` | The model is not Mendix 10 or 11. |
| `invalid_widget_package` | An installed `.mpk` cannot be read. |
| `missing_widget_definition` | No installed package defines a widget the model uses. |
| `ambiguous_widget_definition` | Several installed packages define it. |
| `malformed_widget` | A widget lacks its `Type` or `Object`. |
| `unknown_widget_schema` / `unknown_widget_object` | A widget stores fields neither Mendix nor its package declares. |
| `unknown_property_pointer` | A stored property points at no property type of its schema. |
| `removed_configured_widget_property` | The package dropped a property the widget configures. |
| `malformed_widget_value` | A stored value is not a document. |
| `changed_widget_object` | A configured nested object has no schema to move to. |
| `changed_widget_property` | A configured property changed type with no lossless conversion. |
| `unsafe_layout_weights` | A row's weights cannot be normalized without guessing. |
| `conflicting_design_property` | A legacy spacing option contradicts the compound property. |

`--apply` writes a safe plan in one transaction, each unit only if its
`ContentsHash` is still the one the preview read; otherwise nothing is
written. Running the preview again afterwards reports no changes.

## Parity with mxrb

This is mxrb's `frontend migrate`, with its report, issues and exit codes.
`xtask command-oracle frontend` builds models with mxrb, previews them with
both CLIs (text, JSON and a blocked `--apply`), applies a safe plan with
each to its own copy, and checks that both CLIs then find nothing left in
either copy and that both wrote the same document. Two differences:

- An `invalid_widget_package` message names the zip reader's own error
  after `cannot read widgets/<name>.mpk: `.
- A package property whose `type` is not a pluggable-widget property type
  blocks its widget as `invalid_widget_package`, naming the property; mxrb
  stores such a property with no type at all.
