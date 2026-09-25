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
| `sync` | Applies the model's relational schema to the workspace |
| `explain` | Diagnoses one statement's query plan against the existing indexes |
| `workload` | Reports cumulative query, table and index statistics |
| `indexes` | Proposes index candidates the recorded workload supports |
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

## Schema

`mxrs db sync FILE.mpr [--allow-destructive-schema] [--json]` creates what the
model declares: one table per persistable, non-view entity, one join table per
association, and the `mxrb_schema_*` catalog that records which model artifact
each one belongs to. Running it twice is a no-op — it reports
`Schema already matches the model` and issues no statements at all.

The whole migration goes out as one `psql --command`, which PostgreSQL runs in
a single implicit transaction. It lands whole or not at all.

Anything that would *lose* data is refused by default, naming exactly what
would go:

```console
$ mxrs db sync app/App.mpr
[mxrs] error: schema migration would remove attribute Gone
(mxrb_entity_9ebd….mxrb_attribute_gone); rerun with
--allow-destructive-schema after backing up the database
```

### Columns are typed

| Mendix | PostgreSQL |
| --- | --- |
| String, HashString, Enum | `text` |
| Integer | `integer` |
| Long, AutoNumber | `bigint` |
| Float | `double precision` |
| Decimal | `numeric` |
| Boolean | `boolean` |
| DateTime | `timestamp with time zone` |
| Binary | `bytea` |

The SQLite backend stores everything as `TEXT`/`INTEGER`/`REAL`/`BLOB` because
that layout is a contract with MXRB. PostgreSQL has no MXRB counterpart —
MXRB's `db sync` boots a Mendix Runtime and lets *it* synchronize the schema,
so there is no MXRB code that emits PostgreSQL DDL and nothing to match.

Inheriting SQLite's layout would be worse than pointless. Every query
parameter is bound as a quoted literal, and PostgreSQL resolves an
unknown-typed literal against the column it is compared with. Against
`bigint`, `WHERE n > '99'` finds 100. Against `text`, the same predicate
compares lexicographically, finds nothing, and reports no error — a silently
wrong answer. Typed columns also turn a parameter that cannot be a number into
a loud `invalid input syntax for type bigint`.

`Decimal` is the clearest case: SQLite maps it to `REAL` and loses precision
doing it, and there is no compatibility reason to repeat that here.

An AutoNumber allocates from `mxrb_schema_sequences`, not from a database
identity column — the allocation semantics belong to the model, and every
backend has to agree on them.

### What it does not do

Changing an attribute's default rewrites the column default; it does not
backfill rows that already exist. Adding a *required* attribute to a table
that already has rows, with no default to give them, fails the whole migration
— PostgreSQL refuses the column and the transaction rolls back.

**`db sync` does not make `mxrs serve`'s OQL queries resolve.** The two target
different physical layouts: `sync` writes MXRS's own (`mxrb_entity_<hash>`,
shared with the SQLite backend and with MXRB's `SchemaMigrator`), while
`serve` translates OQL to Mendix Runtime naming (`"Sales$Order"`), which is
what MXRB's OQL server reads because MXRB's `db sync` produces it via the
Runtime. Raw SQL against the tables `sync` creates works today; OQL against
them needs `serve` to be given the physical catalog, which `mxrs-oql` can
already produce (`confidence: "physical"`) but `serve` does not pass. That is
a named gap, not a surprise.

## Cumulative statistics

`mxrs db workload FILE.mpr [--limit N] [--save FILE] [--compare FILE] [--json]`
reads what `pg_stat_statements`, `pg_stat_user_tables` and
`pg_stat_user_indexes` have accumulated since the last statistics reset, and
`mxrs db indexes FILE.mpr [--limit N] [--json]` turns the same reading into
index hypotheses. `--limit` defaults to 20 for `workload` and 50 for
`indexes`, and must be between 1 and 1000.

| Rule | Fires when |
| --- | --- |
| `high_cumulative_time` | A fingerprint spent ≥ 1000 ms in total |
| `high_mean_time` | Its mean execution time is ≥ 100 ms |
| `low_cache_hit` | ≥ 100 shared blocks touched, < 90% of them from cache |
| `temporary_block_writes` | It wrote temporary blocks at all |
| `high_rows_per_call` | It moves ≥ 10 000 rows per call |
| `table_sequential_pressure` | ≥ 100 000 rows read sequentially, and more sequential scans than index scans |
| `unused_large_index` | A non-unique, non-primary index of ≥ 1 MiB with no scans |

Every finding carries the numbers behind it. The statistics are cumulative
over a window this command cannot see the start of, so a threshold is a place
to start looking, not a verdict — which is also why a finding never changes
the exit status.

An index candidate needs two independent signals: a relation under cumulative
sequential-scan pressure, and a column the recorded workload repeatedly
filters on, with either two distinct fingerprints or one that cost ≥ 1000 ms.
Nothing reads a query plan, so a candidate is a hypothesis to test with
`db explain`. The command also names pairs of indexes on one relation whose
column lists are identical after normalization, without picking which to drop.

`--save` writes a versioned snapshot, and `--compare` reports the per-metric
change against one. A positive `change_percent` is a regression; a metric that
was zero and is not any more reports 100%, because a percentage of zero has no
meaning and dropping it would hide a new cost.

### What `db up` has to do for this

`pg_stat_statements` is a preloaded library, so the first `db up` on a
workspace sets `shared_preload_libraries`, restarts the container and waits for
the server again. That happens once per workspace — the setting lives in the
data volume. `track_io_timing` is enabled the same way but only needs a reload,
and the extension itself is created if missing. Every later `db up` pays two
settings queries and one `CREATE EXTENSION IF NOT EXISTS`.

`db up` also waits for the server to accept connections before returning. It
used to return as soon as Docker reported the container started, which is
several seconds earlier: the official image runs a temporary server for
`initdb` and shuts it down again, so readiness needs two consecutive
`pg_isready` probes, not one.

## Not ported

MXRB's `db up` boots a Mendix Runtime container beside PostgreSQL; `db up`
here starts PostgreSQL only. That is also what MXRB's `db sync` is —
`up(force_build: true)`, with the Runtime doing the schema work — so the
`sync` above is MXRS's own answer to the same need, not a port of it.

MXRB's `db workload` also accepts `--engine sql_server`, and exposes the
analyzer's thresholds as a keyword hash it has to validate at runtime. Here the
thresholds are a struct, so an unknown one cannot be written; the CLI does not
expose them, and neither does MXRB's.

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

`Oql::WorkloadAnalyzer`'s and `Oql::IndexAdvisor`'s specs are ported the same
way, every workload threshold is pinned on both sides of its boundary, and the
three catalog queries are asserted against literal SQL rather than against the
constants that produce them — dropping the `pg_stat_statements` exclusion would
let the report rank the query that produced it, and dropping `i.indisprimary`
would turn every primary-key index over 1 MiB into a false `unused_large_index`.

One coercion is worth knowing about: psql renders a small `float8` in exponent
form, which `track_io_timing` makes routine for the I/O columns. MXRB reads
those with `String#to_f` and the row counts with `String#to_i`, and the two
disagree about `8e-05` on purpose — 0.00008 against 8. This reads them the
same way.

The live check goes further: on a 300 000-row table with a duplicated
index and a handful of repeated queries, the three captured statistics
catalogs were fed to MXRB's own analyzers, and both the `workload` and the
`indexes` payloads came out identical to this one — key order included, down
to the nested finding metrics. `db up`'s readiness wait and its monitoring
setup are pinned against a mock Docker, including that the restart happens on
the first run and on no later one.

```sh
cargo test -p mxrs-cli --lib database
cargo test -p mxrs-cli --lib db_reports
cargo test -p mxrs-runtime-postgres
cargo test -p mxrs-oql --lib plan
cargo test -p mxrs-oql --lib workload
cargo test -p mxrs-oql --lib index_advisor
cargo test -p mxrs-oql --lib baseline
mxrs db up app/App.mpr && mxrs db sql app/App.mpr "SELECT count(*) FROM mxrb_schema_entities"
mxrs db explain app/App.mpr "SELECT * FROM mxrb_schema_entities" --analyze
mxrs db sync app/App.mpr
mxrs db workload app/App.mpr --limit 5 --save baseline.json
mxrs db indexes app/App.mpr
```
