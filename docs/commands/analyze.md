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
