# Serve command contract

`mxrs serve FILE.mpr [--port PORT] [--db-port PORT] [--no-up]` ports
`mxrb serve`: a loopback-only JSON API for read-only queries against the
project's owned Docker PostgreSQL workspace (the one `mxrs db` manages).
`--port` defaults to 4567 and `--db-port` to 55432, like mxrb's; `--no-up`
skips starting the database container first.

Every request is `POST` (any path; a different method is
`405 method_not_allowed`, exactly like mxrb's single shared handler) with a
JSON object naming **exactly one** of:

| Field | Behavior |
| --- | --- |
| `sql` | One read-only statement. It must start with `SELECT` or `WITH`, contain no `;` and no NUL byte — enforced before anything reaches psql, with mxrb's own error messages. |
| `oql` | Translated through the safe [`mxrs-oql`] subset (PostgreSQL dialect). An unsupported query returns the translator's warnings as the error message. `params` keys must match the query's `$parameters` exactly (`params must match OQL parameters: …`). |

`params` (optional, JSON object) bind as named `:name` references through
psql variables — never string interpolation. A `null` parameter becomes SQL
`NULL`; strings, numbers and booleans pass through; arrays and objects are
refused (`unsupported parameter value for NAME`). Parameter names must match
`[A-Za-z][A-Za-z0-9_]*`, and `invalid parameter names` / `missing query
parameter` / `unused query parameters` are reported with mxrb's wording. A
`::` cast never counts as a parameter reference.

Responses: `{ok, rows, row_count, elapsed_ms, warnings}` on success — rows
are column-name maps built from psql's CSV output, where an unquoted empty
field is SQL `NULL` (JSON `null`) and a quoted `""` stays an empty string —
or `{ok: false, error: {code, message}, elapsed_ms}` with
`invalid_request` (400), `query_failed` (422), `invalid_json` (400),
`request_too_large` (413, bodies over 1 MiB) and `method_not_allowed`
(405).

## What differs from mxrb

mxrb enforces read-only access with a dedicated PostgreSQL reader role;
this workspace has a single owner role, so the same guarantee comes from
`PGOPTIONS=-c default_transaction_read_only=on` on every query plus the
single-statement validation. `explain`/`workload` analysis and the Mendix
Runtime container mxrb's workspace also manages are not part of this
command.
