# UML export contract

`mxrs uml FILE.mpr --export class|activity|sequence [--format mermaid|plantuml]
[--module NAME] [--microflow Module.Flow] [--root NAME] [--depth N]` prints the
same diagram text as native `mxrb uml --export`, byte for byte. The oracle
(`cargo run -p xtask -- command-oracle uml <mxrs-binary>`) compares every
export/format combination over disposable native models in both storage
formats and asserts shared refusals for the same invalid inputs.

| Export | Source of truth | Selection |
| --- | --- | --- |
| `class` | domain model entities, attributes, associations | all modules, or repeated `--module NAME` filters |
| `activity` | one microflow's native object/sequence-flow graph | `--microflow Module.Flow` (required) |
| `sequence` | `calls` references from the document index the verified `callees`/`callers` commands use | exactly one of `--root NAME` (call chain, `--depth N`, default 2, max 100) or `--module NAME` (intra-module edges) |

Formatting rules are ported from `Mxrb::Uml::Support`: deterministic
identifiers (sanitized name plus the first 8 hex characters of its SHA-256
when sanitization changed it), Mermaid HTML-escaping, PlantUML
backslash/quote escaping, and camel-case word splitting for action type
labels. Class stereotypes mark OQL views (`OQL View`) and non-persistable
entities (`DTO`); enumeration attributes render the enumeration's short name.

## What is not ported

The interactive loopback viewer (`mxrb uml FILE.mpr` without `--export`,
`uml/server.rb` plus the bundled web UI) is a browser subsystem and is not
implemented. MXRS refuses viewer mode with an explicit error naming
`--export` instead of serving; `--port` is accepted and ignored during
export, exactly as native ignores it there. The `/modeler` project editor
that viewer hosts is likewise out of scope.
