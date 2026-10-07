# `mxrs diagram-er`

```sh
mxrs diagram-er FILE.mpr [--module NAME]... [--output FILE] [--port PORT] [--force]
mxrs diagram-er up FILE.mpr [--module NAME]... [--output FILE] [--port PORT] [--force] [--json]
mxrs diagram-er status|down FILE.mpr [--json]
mxrs diagram-er destroy FILE.mpr --yes
mxrs diagram-er FILE.mpr [--module NAME]... --json
mxrs diagram-er layout FILE.mpr LAYOUT.json [--apply] [--json]
```

The domain model as an editable ER diagram in the browser, as mxrb's.

The editor works on a copy of the model — `FILE.domain-layout.mpr` beside
it unless `--output` names another, with its `mprcontents` — and never on
the model itself; an existing copy is replaced only with `--force`. It is
served on loopback only (port 4568 unless `--port`): the page, its assets,
`GET /api/diagram`, and `POST /api/layout`, which takes entity positions and
association anchors signed with the token the page was given (the editor is
mxrb's, so its header is `X-MXRB-Token`), at most 2 MiB, into the copy.
"Exportar PNG" in the page saves the diagram as an image.

`up` runs the editor detached and answers once it serves; `status` and
`down` reach it through an endpoint only the holder of its lifecycle token
may call, rather than a PID that may be stale; `destroy --yes` stops it and
removes the copy and its state. State lives in
`~/.local/state/mxrs/diagram-er` (`MXRS_DIAGRAM_STATE_ROOT`), one private
folder per model, with the server's log.

`--json` without a lifecycle action prints the diagram as the editor reads
it, and `layout` applies a layout from a file — the same audited writer.
