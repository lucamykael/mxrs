# Native document queries

These commands implement the native MXRB document query contracts. All accept
`--json`, `--no-progress`, `--help` and `-h`.

| Command | Successful human output |
| --- | --- |
| `callers FILE.mpr NAME` | Distinct direct callers: qualified name, tab, kind |
| `callees FILE.mpr NAME` | Distinct directly called artifacts, same format |
| `refs FILE.mpr NAME` | Incoming source name, kind, relation and dotted property path, separated by tabs |
| `impact FILE.mpr NAME` | Transitive incoming dependencies in discovery order, name and kind |
| `describe FILE.mpr NAME` | Name/kind, then `Incoming:` and `Outgoing:` reference lists |
| `tree FILE.mpr [MODULE]` | Modules, kinds and sorted qualified names, indented as in MXRB |

Human output matches native output byte for byte. Successful queries exit 0,
including empty results. An unknown or ambiguous artifact fails with stderr
and no report. Artifact arguments are exact, case-sensitive qualified names;
bare names and IDs are not aliases. An unmatched tree module filter produces
an empty result, as in MXRB. Invalid MPRs, corrupt BSON and invalid/extra/repeated
options fail. Ruby exception stack traces are not an output contract.

## Inventory and references

The document index includes modules, entities, attributes, associations and
every named unit, including unknown document families and nested folders.
Attribute names use `Module.Entity.Attribute`. Navigation uses
`Project.Navigation`. Module ancestry follows containers and terminates on
missing or cyclic parents. Nameless units are omitted except navigation.

Reference scanning follows native document and property encounter order. It
keeps full paths, including stored array indices, and scans direct strings,
qualified tokens in expressions and `Module.Entity/Attribute` member tokens.
Names without a module prefix can resolve within the source module. Ambiguous
references use the property's expected kind, then a unique local match.
Metadata fields and self-identity fields follow native exclusions. The same
target at two different paths remains two references; callers/callees deduplicate
by artifact ID while keeping first encounter order. Impact traverses incoming
edges breadth first and excludes the root even in a cycle.

This is native textual reference discovery, not expression type checking or a
proof that every reference is executable. Unresolved strings do not become
dependencies. The compiler/refactoring graph retains its separate explicit
reference rules: textual matches are insufficient evidence for rewriting a
model. `lint`, `report`, refactoring, search and the existing typed semantic
cache retain their documented scope. These six browse commands build a fresh
read-only document index and never write caches into the MPR or filesystem.

## JSON extension

Native MXRB has no JSON mode for these commands. MXRS provides:

- Callers, callees and impact: artifact arrays in the same order as human output.
- Describe: `{artifact, incoming, outgoing, fingerprint}`.
- Refs: `{artifact, incoming, fingerprint}`. Only incoming references belong
  to this command; describe provides both directions.
- Tree: an object mapping module names to kinds and sorted qualified names;
  project-level artifacts use `(project)`.

Artifacts expose native ID, qualified name, kind, module/name, unit ID, path and
portable metadata. IDs retain their native `module:`, `entity:`, `attribute:`,
`association:` or `unit:` prefix. Ruby model objects are not serialized.
References have `from`, `to`, `relation`, `path` and original string `value`.
Paths are arrays of strings, including array positions, matching native
reference paths. The fingerprint is an MXRS SHA-256 of the document graph.

## Repeat verification

```sh
cargo build -p mxrs-cli -p xtask
target/debug/xtask command-oracle callers target/debug/mxrs
target/debug/xtask command-oracle callees target/debug/mxrs
target/debug/xtask command-oracle refs target/debug/mxrs
target/debug/xtask command-oracle impact target/debug/mxrs
target/debug/xtask command-oracle describe target/debug/mxrs
target/debug/xtask command-oracle tree target/debug/mxrs
```

Each oracle executes both real CLIs and checks exact human output plus complete
portable native facts in JSON. Disposable fixtures cover v1/v2, unknown and
project-level documents, nested folders, associations, repeated calls, expression
members, case sensitivity, ambiguity, cycles, row reordering, unnamed modules,
empty projects, corrupt storage and invalid arguments. Hashes and file
inventories attest that all queries are read-only. Rust regressions check
reference paths, kind disambiguation and cyclic impact independently.
