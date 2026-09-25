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
| `explain` | Diagnoses one statement's query plan against the existing indexes |
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

## Query plans

`mxrs db explain FILE.mpr "SELECT ..." [--analyze] [--json]` asks PostgreSQL
for `EXPLAIN (FORMAT JSON, COSTS, VERBOSE, SETTINGS)` and turns the plan into
conservative findings. The statement must be one read-only `SELECT` or `WITH`,
the same rule online queries follow.

| Rule | Fires when | Severity |
| --- | --- | --- |
| `sequential_scan` | A `Seq Scan` appears | Warning at ≥ 1000 rows or ≥ 1000 cost, otherwise a hint |
| `filter_discard` | A filter removed ≥ 1000 rows and more than it returned | Warning |
| `cardinality_misestimation` | Estimated and actual rows differ by ≥ 10x, with ≥ 100 rows on one side | Warning |
| `high_volume_nested_loop` | A `Nested Loop` processes ≥ 10 000 rows across its loops | Warning |
| `disk_sort` | A `Sort` spilled to disk or used an external method | Warning |

A sequential scan is not automatically a problem: scanning a small relation is
often cheaper than an index lookup, so a small scan is reported as a hint and
leaves the report `clean`. No finding invents a column recommendation — it
names the relation, the filter, and the indexes `pg_indexes` already has for
that relation, in that relation's schema when the planner named one.

`--analyze` adds `ANALYZE`, `BUFFERS` and `TIMING`, which means the statement
actually runs. MXRB keeps that safe by connecting as a reader role; here the
session carries `default_transaction_read_only`, which covers the case the
statement check cannot: a data-modifying CTE
(`WITH gone AS (DELETE FROM orders RETURNING *) SELECT * FROM gone`) starts
with `WITH`, and it is the server setting that refuses to execute it.

Findings never change the exit status. A plan diagnosis is advice about a
query that ran, not a failure of the command.

## Not ported

MXRB's `db` also offers `sync`, `workload` and `indexes`, and its `up` boots a
Mendix Runtime container beside PostgreSQL. None of that is here:

- `sync` — schema synchronization against PostgreSQL. The relational migrator
  exists (`mxrs-runtime-sqlite`'s `schema.rs`) but targets SQLite.
- `workload` / `indexes` — need `Oql::WorkloadAnalyzer` and `Oql::IndexAdvisor`
  over `pg_stat_statements`.
- Mendix Runtime boot — `db up` here starts PostgreSQL only.

MXRB's `db explain` also accepts `--engine sql_server`. This one is PostgreSQL
only, and the report names its engine rather than leaving it implied.

One deliberate near-miss: the filter quoted inside a scan suggestion uses
Rust's `Debug` escaping where MXRB uses `String#inspect`. They agree on the
ASCII predicates PostgreSQL renders in practice, but not in general — Ruby
escapes `#{`, and Rust escapes combining marks. Only that one sentence of one
suggestion is affected.

## Verification

The psql argv for `sql`, `explain` and `shell` is pinned against a mock Docker,
including that read-only is the default, that `--write` removes exactly the
read-only setting and nothing else, that `--analyze` does not remove it, that
the statement reaches psql verbatim, and that the two failure classes stay
distinct.

`Oql::PlanAnalyzer`'s own spec is ported case for case: the five rules firing
together on one plan, a small scan staying a hint, a large unindexed scan
suggesting nothing it cannot support, external sorts against low-volume noise,
and both malformed-payload rejections. The `--json` payload is pinned too,
including that `clean` is derived from the findings and that a metric the plan
does not carry is omitted rather than serialized as a zero. Every threshold is
pinned on both sides of its boundary, and the two conditions guarding a single
rule are separated so that neither can carry the other.

The text `db explain` prints is rendered by a library function and asserted
whole, including the empty subject MXRB prints for a plan node that has
neither a relation nor a node type. The command grammar is asserted too: which
actions take a statement, and which action each flag belongs to.

Live-checked against a real workspace: a `SELECT` renders as psql renders it,
`CREATE TABLE` is refused without `--write` and accepted with it, and the
resulting table is then visible to a read-only query. For `explain`, on a
20 000-row table with one index: a filtered scan reports the warning and lists
that index, and under `--analyze` a data-modifying CTE is refused by the
read-only session with every row still present afterwards. The same captured
`EXPLAIN` JSON and `pg_indexes` rows fed to MXRB's own `PlanAnalyzer` produce
an identical `--json` payload, key order included.

```sh
cargo test -p mxrs-cli --lib database
cargo test -p mxrs-cli --lib db_reports
cargo test -p mxrs-oql --lib plan
mxrs db up app/App.mpr && mxrs db sql app/App.mpr "SELECT count(*) FROM mxrb_schema_entities"
mxrs db explain app/App.mpr "SELECT * FROM mxrb_schema_entities" --analyze
```
