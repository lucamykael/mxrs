# Database command contract

`mxrs db <action> FILE.mpr [--port PORT] [--json]` manages a PostgreSQL
workspace that belongs to one project and nothing else.

| Action | What it does |
| --- | --- |
| `up` | Creates or starts the container and volume, writing state and a password under the state directory |
| `status` | Reports the container, volume, image, port and whether the workspace was initialized |
| `down` | Stops the container, preserving the volume |
| `destroy` | Removes container, volume and local state |
| `credentials` | Prints host, port, database, user and password |
| `url` | Prints the `postgresql://` connection URL |
| `sql` | Runs one statement and prints psql's own rendering |
| `shell` | Hands the terminal to an interactive `psql` |

## Isolation

The container and volume names are derived from a hash of the model's
absolute path, so two projects never share a database. Both carry
`io.mxrs.managed` and `io.mxrs.project` labels, and every operation refuses to
touch a Docker object that is not labelled for this project — an unowned
container is never started, stopped or removed.

The port is published on loopback only.

## Read-only by default

`db sql` and `db shell` connect read-only unless `--write` is given.

MXRB reaches this by connecting as a dedicated reader role and wrapping the
statement in `BEGIN READ ONLY` / `COMMIT`. This workspace has a single owner
role, so the guarantee comes from `default_transaction_read_only`, passed as
`PGOPTIONS` when the session is created. The difference matters: a session
setting applies to every transaction the session opens, so no statement inside
the payload can turn it off, and a multi-statement `--command` cannot escape it
by opening its own transaction.

```console
$ mxrs db sql app/App.mpr "DROP TABLE orders"
[mxrs] error: ERROR:  cannot execute DROP TABLE in a read-only transaction
```

`--write` is accepted only by `sql` and `shell`; on any other action it is an
error rather than a silently ignored flag.

## Diagnostics

`docker exec` passes the container command's exit status through, so a failed
statement and an unreachable container both arrive as a non-zero exit. They are
told apart rather than merged: stderr carrying a PostgreSQL diagnostic
(`ERROR:`, `FATAL:`, `psql:`) is reported as a query failure, and anything else
keeps the operational "Docker is unavailable" wording. Reporting a rejected
statement as a Docker outage blames the wrong component for something the
caller can fix.

An empty statement, or one containing a NUL byte, is refused before Docker is
invoked at all.

## Not ported

MXRB's `db` also offers `sync`, `explain`, `workload` and `indexes`, and its
`up` boots a Mendix Runtime container beside PostgreSQL. None of that is here:

- `sync` — schema synchronization against PostgreSQL. The relational migrator
  exists (`mxrs-runtime-sqlite`'s `schema.rs`) but targets SQLite.
- `explain` — needs a port of `Oql::PlanAnalyzer` (185 lines) plus its plan
  rendering.
- `workload` / `indexes` — need `Oql::WorkloadAnalyzer` and `Oql::IndexAdvisor`
  over `pg_stat_statements`.
- Mendix Runtime boot — `db up` here starts PostgreSQL only.

## Verification

The psql argv for `sql` and `shell` is pinned against a mock Docker, including
that read-only is the default, that `--write` removes exactly the read-only
setting and nothing else, that the statement reaches psql verbatim, and that
the two failure classes stay distinct.

Live-checked against a real workspace: a `SELECT` renders as psql renders it,
`CREATE TABLE` is refused without `--write` and accepted with it, and the
resulting table is then visible to a read-only query.

```sh
cargo test -p mxrs-cli --lib database
mxrs db up app/App.mpr && mxrs db sql app/App.mpr "SELECT count(*) FROM mxrb_schema_entities"
```
