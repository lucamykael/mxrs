//! The library half of the `mxrs` binary — a Phase 6 CLI skeleton per the
//! project's phased roadmap ("`mxrs-cli` skeleton (validate/compare/export)
//! can start as early as end of Phase 3"). Split into a lib (this crate) +
//! thin `main.rs` dispatcher so every subcommand is directly testable
//! without shelling out to the built binary.
//!
//! **`export`'s meaning changed from the original plan.** `mxrb export` is
//! `Exporter` (model → **Ruby**-DSL source) — still out of scope, the
//! project's locked scope drops Ruby-source generation entirely (see
//! `decisions/mxrs-rust-rewrite-plan.md`). `mxrs export` means something
//! different and new instead: model → **Rust** `project! {}` source, via
//! `mxrs-exporter` — the forward half of the "open an existing Mendix
//! project as editable Rust, change it, write it back" round trip (the
//! backward half already existed: `mxrs-writer`'s `synchronize_project`).
//!
//! **`inspect`** — `compare`'s `snapshot` function already builds a full
//! structural summary of *one* project (it's called twice, once per side,
//! before diffing); exposing it standalone as `mxrs inspect <file.mpr>`
//! needed no new model-reading logic, just a new front end onto what
//! `compare` already had.
//!
//! **`units`/`dump-unit`/`sql`/`modules`** — port `bin/mxrb`'s commands of
//! the same purpose (`bin/mxrb`'s own command is named `inspect`; renamed
//! `units` here since `mxrs inspect` already means the structural summary
//! above, a more useful default for that name). See `browse`'s module doc.

//! **Artifact scaffolding** (`entity`/`enumeration`/`use-case`/`page`/
//! `nanoflow`/`published-rest`/`consumed-rest`/`java-action`/`security`/
//! `module`/`scaffold`/`project`) ports mxrb's source generators, which write
//! declaration source into a project rather than touching an `.mpr`. See
//! `scaffold`'s module doc for the contract kept and `mxrs-scaffold`'s
//! `artifact` module for what necessarily differs between generating Ruby that
//! is evaluated at run time and Rust that has to compile.
//!
//! **Semantic refactoring** (`rename`/`remove`/`move`) ports mxrb's commands
//! of the same names. Unlike the generators these mutate an `.mpr` directly,
//! so they preview by default and write only under `--apply` — see
//! `refactor`'s module doc and `mxrs-refactor`'s crate doc for what a rename
//! can and cannot see.

pub mod arguments;
pub mod browse;
pub mod cargo_project;
pub mod compare;
pub mod inspect;
pub mod refactor;
pub mod scaffold;
pub mod validate;
