//! The library half of the `mxrs` binary — a Phase 6 CLI skeleton per the
//! project's phased roadmap ("`mxrs-cli` skeleton (validate/compare/export)
//! can start as early as end of Phase 3"). Split into a lib (this crate) +
//! thin `main.rs` dispatcher so `validate`/`compare`/`inspect` are directly
//! testable without shelling out to the built binary.
//!
//! **`export` is deliberately not ported in this pass.** mxrb's own
//! `mxrb export` is `Exporter` (model → **Ruby**-DSL source), which the
//! project's locked scope drops entirely (no Rust equivalent — see
//! `decisions/mxrs-rust-rewrite-plan.md`); a hypothetical `mxrs export`
//! would more plausibly mean deployment packaging (MDA/portable ZIP), which
//! depends on `mxrs-packager`/`mxrs-materializers` (Phase 5, not built yet).
//! Revisit once Phase 5 lands and it's clear what "export" should mean here.
//!
//! **`inspect` is added in this pass instead** — `compare`'s `snapshot`
//! function already builds a full structural summary of *one* project (it's
//! called twice, once per side, before diffing); exposing it standalone as
//! `mxrs inspect <file.mpr>` needed no new model-reading logic, just a new
//! front end onto what `compare` already had.

pub mod compare;
pub mod inspect;
pub mod validate;
