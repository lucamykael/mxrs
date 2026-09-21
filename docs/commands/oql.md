# OQL projection

`mxrs oql FILE.mpr [--dialect postgresql|sql_server|ansi] [--json]` discovers
native OQL view sources and projects the safe read-only subset to the physical
table and column names used by MXRS's SQLite runtime. Its output is an
inspection aid; it never executes SQL.

The project-aware form resolves stable storage identities, including an
association path in an aliased join:

```oql
SELECT c/Name
FROM Sales.Order o
JOIN o/Sales.Order_Customer c
```

The path expands to the runtime association table and the target entity table.
Both forward and reverse traversal are supported. A path must use a known
`source_alias/Module.Association` and must not add an explicit `ON` condition;
the association supplies the only safe condition. Unsupported source-path
forms, unknown aliases, non-persistable entities, and unknown attributes are
reported as unsupported rather than guessed.

`mxrs translate-oql QUERY [--dialect ...]` is intentionally metadata-free. It
returns a logical projection and continues to reject association paths because
it cannot safely infer a runtime storage mapping without an MPR.
