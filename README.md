# mxrs

A ground-up Rust implementation of Mendix Studio Pro `.mpr` project reading,
writing, compiling and packaging — no Ruby, no mxcli, single static binary.

Sibling project to [mxrb](https://github.com/lucamykael/mxrb) (Ruby), which
remains the reference implementation and behavioral oracle during
development, but is not a runtime dependency of anything shipped here.

The full phased implementation plan (crate layout, dependency DAG, milestone
done-definitions, DSL/macro design, testing strategy) lives in this
project's `ai-memory` as `decisions/mxrs-rust-rewrite-plan.md`.

## Status

Phase 1 (binary I/O foundation) in progress. See `crates/`:

- `mxrs-bson` — Mendix-specific BSON codec (MS-GUID conversion, `$ID`
  extraction, array-marker convention). Ports `lib/mxrb/io/bson_codec.rb`.
- `mxrs-mpr`, `mxrs-fragment-store`, `mxrs-schema` — skeletons, not yet
  implemented.

## Development

```sh
cargo build
cargo test
cargo clippy --all-targets
```
