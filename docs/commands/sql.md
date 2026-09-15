# SQL command contract

`mxrs sql FILE.mpr "QUERY" [--no-progress]` executes a read-only SQLite query.
The MXRB counterpart is `mxrb sql FILE.mpr "QUERY"`.

## Results

Every row is printed as `[cell, cell, ...]`, followed by `(N rows)`. Row and
column order, dynamic types and original bytes are preserved. Empty results
print `(0 rows)`. Placeholders without bindings have SQLite's NULL value.

| SQLite value | MXRS presentation | MXRB presentation |
| --- | --- | --- |
| NULL | `NULL` | `nil` |
| Integer | Signed decimal, including all 64-bit values | Same |
| Real | Round-trippable float, including `1.0` and `inf` | Ruby float notation |
| UTF-8 text | JSON-quoted string with control characters escaped | Ruby inspected string |
| Blob | Complete `X'hexbytes'` | Ruby binary string |
| Invalid UTF-8 text | Complete `TEXT X'hexbytes'` | Ruby string with escaped invalid bytes |

For example, `SELECT 'a,b', X'00ff', CAST(X'80' AS TEXT), NULL` prints
`["a,b", X'00ff', TEXT X'80', NULL]`. Text, blobs and invalid text remain
unambiguous; no byte is replaced by a Unicode replacement character or a blob
length summary. The output is a diagnostic format, not JSON or executable SQL.

## Read-only and error behavior

Inline v1 and external v2 MPR stores are supported. Queries use the original
SQLite schema; no BSON decoding, compilation, cache warming or mutation occurs.
Recursive CTEs, SELECT functions and supported introspective PRAGMAs work.

MXRS rejects writes, temporary DDL, ATTACH/DETACH, VACUUM INTO, transaction
control, state-changing PRAGMAs and external-file/extension functions. This is
stricter than SQLite's read-only database flag and MXRB's native connection.
Multiple SQL statements are rejected; MXRB may execute only the first. Empty
or comment-only input, invalid SQL and invalid paths/databases fail. Successful
row collection finishes before stdout begins, so errors produce no partial rows.

`--no-progress` is accepted as a compatibility flag. Both help forms work.
Unknown/repeated options and extra arguments fail instead of being ignored.
Diagnostic wording and Ruby stack traces are not reproduced.

## Repeat verification

```sh
cargo build -p mxrs-cli -p xtask
target/debug/xtask command-oracle sql target/debug/mxrs
```

The permanent oracle executes both real CLIs against disposable MXRB-created
v1/v2 stores. It checks native CLI output against the native query values, then
compares MXRS cells using only the presentation mapping above (numeric equality
for float notation). It covers every value type, invalid UTF-8, escapes, signed
integer extremes, infinity, zero/multiple rows, recursive queries, schema
PRAGMAs, unbound parameters, errors, argument handling and stricter read-only
refusals. Hashes and file inventories must remain unchanged. Rust regression
tests additionally recover all 256 possible byte values from CLI output.

Verified refers to this diagnostic command, not OQL or runtime database access.
