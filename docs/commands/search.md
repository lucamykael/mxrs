# `mxrs find` and `mxrs search`

```sh
mxrs find <file.mpr> <text> [--semantic]
mxrs search <text> <file.mpr> [--backend auto|tfidf] [--limit N] [--json]
```

Both read the project's semantic index (the artifacts `mxrs tree` lists)
and never write the `.mpr`.

## `find`

Without `--semantic`, `find` lists every artifact whose qualified name holds
`text`, whatever its case, sorted by module, kind and qualified name — one
`qualified_name<TAB>kind` line each. With `--semantic` it lists the ten
artifacts `search` ranks nearest the text, in that order.

## `search`

`search` ranks every artifact by how near its text is to `text`, and prints
the first `--limit` (10 by default):

```text
rank	distance	qualified_name	kind
1	0.292893	Sales.ACT_Order_Save	microflow
```

An artifact's text is its qualified name, name, kind, module and
documentation. Each text is a vector of 512 buckets: its lowercase ASCII
words of two characters or more, each counted into the bucket its 32-bit
FNV-1a hash names, weighted by its share of the words and scaled to unit
length. The distance is one minus the cosine similarity of the two vectors;
ties are broken by qualified name. `--json` prints the same ranking as an
array of `{rank, qualified_name, kind, distance}`.

This is mxrb's TF-IDF backend, to the last printed digit (its sums are
compensated as Ruby's are), and `xtask command-oracle find|search` runs both
CLIs on the same models. mxrb's `auto` backend uses an ONNX sentence model
when its optional gems are installed; mxrs has no such model, so `auto` is
the TF-IDF backend and `--backend onnx` is refused by name.
