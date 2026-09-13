# Workspace layers

The workspace is grouped by dependency direction. Crate package names remain
unchanged; only their repository paths encode architectural ownership.

- `io/`: byte/storage boundaries (`mxrs-bson`, `mxrs-mpr`, fragment storage).
- `model/`: Mendix schemas, codecs, settings, pluggable widgets, and the read model.
- `authoring/`: storage-independent IR, typed expressions/DSL, macros, and type generation.
- `writer/`: stable identities and `.mpr` synchronization/materialization.
- `compiler/`: runtime-model and generated-code compilers.
- `app/`: project orchestration, Cargo export/import, CLI, and the public facade.

Dependencies should point inward/downward. Cross-layer cycles are not allowed;
`cargo metadata` and the workspace test suite are the executable dependency map.
Developer automation remains in the root `xtask/` crate.
