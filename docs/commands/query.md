# `mxrs query` and `mxrs oql`

```sh
mxrs query "SELECT ..." --from sql|oql [--to oql|sql] [--dialect ansi|postgresql|sql_server] [--project FILE.mpr] [--input FILE|-] [--json]
mxrs oql <file.mpr> [--dialect ansi|postgresql|sql_server] [--layout logical|physical] [--json]
```

Both are mxrb's commands, and `xtask command-oracle query|oql` compares the
two CLIs' text, warnings, JSON and exit codes.

## OQL to SQL

OQL becomes the SQL of a database the Mendix Runtime made, with its names
inferred and said to be: a table is `"sales$order"` (`[sales$order]` in SQL
Server, `"Sales.Order"` in ANSI), an attribute its lowercase name, a
parameter `:name`. A query that opens with its `FROM` is written `SELECT`
first. An association path needs storage metadata a model lacks and is
refused. Only one read-only statement is taken.

`mxrs oql` does this for every OQL string the model stores (the catalog
`mxrs analyze` reads). `--layout physical` projects onto the tables mxrs's
own runtime stores instead, where an aliased association-path `JOIN`
expands through its storage identities.

## SQL to OQL

`query --from sql` takes one read-only `SELECT` back to logical OQL: a
source is `Module.Entity`, or the Runtime's physical `module$entity` (a
`public.`/`dbo.` schema dropped), an attribute path `alias/Name`, `<>` is
`!=`, `/` is `:`, `:name` and `@name` are `$name`, `LEN` and `CHAR_LENGTH`
are `LENGTH`. Without `--project`, a physical name's casing is inferred and
the projection is `inferred`; with it, entities and attributes take the
model's casing. What OQL has no safe form for — a `WITH`, a write, `::`
casts, `ILIKE`, `DISTINCT ON`, a table subquery, a function OQL lacks,
concatenation and bitwise operators, positional parameters — is refused with
the reason, and the command exits 1.
