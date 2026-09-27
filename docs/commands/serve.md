# Serve command contract

`mxrs serve FILE.mpr [--port PORT] [--db-port PORT] [--no-up]
[--oql-layout auto|physical|mendix]` ports `mxrb serve`: a loopback-only JSON
API for read-only queries against the project's owned Docker PostgreSQL
workspace (the one `mxrs db` manages). `--port` defaults to 4567 and
`--db-port` to 55432, like mxrb's; `--no-up` skips starting the database
container first.

## Which tables OQL translates to

Two legitimate physical layouts exist, and OQL against the wrong one fails
every query with `relation ... does not exist`. A database `mxrs db sync`
wrote has MXRS's own typed layout (`mxrb_entity_<hash>`); a database the
Mendix Runtime synchronized has Mendix Runtime naming (`"Sales$Order"`) — the
only one mxrb's server knows. `serve` resolves the choice once, before
accepting requests, and announces it on the startup banner:

- `--oql-layout auto` (the default) probes the database for the
  `mxrb_schema_*` catalog that only MXRS's own schema applier creates:
  present means physical, absent means Mendix Runtime naming. The probe reads
  the database, so with `--no-up` it needs the container already running; when
  it cannot answer, `serve` refuses to start rather than guess a layout.
- `--oql-layout physical` targets `db sync`'s tables outright, deriving the
  catalog from the model once at startup (`confidence: "physical"`, with
  association-path `JOIN`s expanded from real storage identities).
- `--oql-layout mendix` is mxrb's behavior, verbatim.

The layout only affects `oql` requests; raw `sql` always reaches the database
as written.

Every request is `POST` (any path; a different method is
`405 method_not_allowed`, exactly like mxrb's single shared handler) with a
JSON object naming **exactly one** of:

| Field | Behavior |
| --- | --- |
| `sql` | One read-only statement. It must start with `SELECT` or `WITH`, contain no `;` and no NUL byte — enforced before anything reaches psql, with mxrb's own error messages. |
| `oql` | Translated through the safe [`mxrs-oql`] subset (PostgreSQL dialect). An unsupported query returns the translator's warnings as the error message. `params` keys must match the query's `$parameters` exactly (`params must match OQL parameters: …`). |

`params` (optional, JSON object) bind as named `:name` references through
psql variables — never string interpolation. A statement carrying bound
parameters reaches psql on stdin, because psql's `--command` performs no
variable interpolation at all; one without them keeps `--command`. A `null`
parameter becomes SQL
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

With raw `sql`, mxrb's server silently discards `params`; mxrs binds and
validates them exactly as it does for OQL — strictly more useful, and a
malformed set fails loudly instead of vanishing.

mxrb's server reads only a database the Mendix Runtime created, so it has no
layout decision to make; `--oql-layout` and the startup probe are MXRS's own,
made necessary by `db sync` writing a layout of its own.

mxrb enforces read-only access with a dedicated PostgreSQL reader role;
this workspace has a single owner role, so the same guarantee comes from
`PGOPTIONS=-c default_transaction_read_only=on` on every query plus the
single-statement validation. `explain`/`workload` analysis and the Mendix
Runtime container mxrb's workspace also manages are not part of this
command.
