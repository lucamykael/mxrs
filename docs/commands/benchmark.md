# Benchmark command contract

`mxrs benchmark FILE.mpr [--iterations N] [--json]` measures the three
read-only operations used by `mxrb benchmark`: opening and enumerating the
model units, constructing a semantic index, and validating storage integrity.

`N` defaults to `3` and must be an integer from `1` through `100`. The command
does not modify the model. Semantic-index timing always builds a fresh index;
it deliberately bypasses MXRS's derivative cache, so a prior `mxrs cache warm`
cannot change the work being measured.

## Output

The human output uses MXRB's four labels:

```text
Units            : 8
Open average     : 0.000000s
Index average    : 0.000000s
Validate average : 0.000000s
```

`--json` emits the same five result fields as MXRB:
`iterations`, `open_seconds`, `index_seconds`, `validate_seconds`, and
`units`. Durations are wall-clock averages in seconds and are intentionally
not compared numerically between invocations or implementations.

All work for a phase completes before the next phase starts. Opening, semantic
indexing, or storage failures therefore produce no benchmark report; the error
names the failed phase. Invalid options, duplicate options, extra arguments,
and a missing option value fail before opening the model.

## Verification

The CLI contract suite creates an authored disposable MPR and asserts the
default/explicit iteration grammar, range rejection, missing-model failure,
human labels, and typed JSON result fields. A direct MXRB/MXRS run over the
redistributable minimal MPR confirms the shared field shape and unit count;
durations remain observational measurements rather than fixture constants.

```sh
cargo test -p mxrs-cli --test commands benchmark
cargo run -p mxrs-cli --bin mxrs -- benchmark xtask/fixtures/minimal/minimal.mpr --iterations 1 --json
../../mxrb/bin/mxrb benchmark xtask/fixtures/minimal/minimal.mpr --iterations 1 --json
```
