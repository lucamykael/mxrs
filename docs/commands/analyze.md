# Query analysis

`mxrs analyze` reports positional, read-only query risks. It accepts either a
native model or one explicit source:

```sh
mxrs analyze App.mpr --dialect postgresql
mxrs analyze --oql "SELECT * FROM Sales.Order o WHERE o/Name LIKE '%term%'" --json
mxrs analyze --sql "SELECT * FROM sales_order" --dialect sql_server
```

The analyzer reports leading, both-side and trailing `LIKE` wildcards,
functions in `WHERE`, comma joins without `JOIN`, and `SELECT *`. Every
finding includes the PostgreSQL, SQL Server and ANSI suggestion set; human
output selects the requested dialect. This is a risk report, not an execution
plan or a complete Studio Pro consistency check.

Without `--sql` or `--oql`, the queries are every OQL string the model
stores, as mxrb's catalog finds them: a node whose type names OQL and holds a
`Query` (a data set's source), a view entity's source document, and any
`OqlQuery` field. Each is named for the nearest named document holding it,
in its module, and has a kind — `dataset`, `view_entity` or `oql`; a query
given on the command line is `AdHoc`, of kind `oql` or `sql`.

Each rule is mxrb's, applied as its expressions apply: every `LIKE` literal,
each `LOWER`/`UPPER`/`CAST` call inside a `WHERE` clause up to its
`GROUP BY`/`ORDER BY`/`HAVING`/`LIMIT`/`OFFSET`/`UNION`, a `FROM` list holding
a comma and no `JOIN`, and each `*` a `SELECT` projects (an aggregate's
`COUNT(*)` excepted). A finding keeps the fragment it is about. The text
output is one block per finding:

```text
Sales.View0  [ERROR] cartesian_join
  VIEW_ENTITY: FROM Sales.Order o, Sales.Line l
  Comma-separated entities without an explicit JOIN risk a Cartesian product.
  PostgreSQL -> Use an explicit JOIN with an ON predicate.
```

`--json` lists each query with `name`, `kind`, `source`, `clean` (no error),
`warnings` (some warning) and its findings, each with every dialect's
suggestion and the `selected_suggestion` of the requested dialect.
`xtask command-oracle analyze` runs both CLIs over the same model and
queries.
