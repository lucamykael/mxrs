# MXRS engineering contract

This repository implements MXRS only. Treat the sibling MXRB repository as a
read-only behavioral oracle; never edit it.

## Product standard

- Editable generated Rust is a first-class product surface. It must be
  idiomatic, concise, expressive, discoverable, warning-free, and organized by
  clean architecture. Exploit Rust's type system to make invalid Mendix models
  difficult or impossible to express.
- Zero known errors, silent loss, misleading diagnostics, opaque generated
  Rust, placeholder implementations, or untracked partial behavior is the
  completion standard. Preserve unsupported native data losslessly outside the
  editable Rust tree only as a temporary compatibility boundary, then add a
  typed DSL/IR/writer/exporter surface for it.
- Generated projects separate domain declarations, application/server
  microflows, presentation/client nanoflows and pages, and infrastructure
  adapters. Dependencies point inward.
- Scaffolds must compile immediately, be transactional, refuse overwrites, and
  be configurable through explicit command options and project defaults.
- Never commit private acceptance projects, their names, paths, metrics, or
  identifying references. Tests and documentation use neutral corpus names and
  environment variables. Public fixtures must be redistributable.
- Never commit credentials. Platform access commands store only references to
  secrets and enforce private filesystem permissions.

## Increment protocol

1. Read `decisions/mxrs-open-backlog.md` from the current project's ai-memory
   before non-trivial work. Update that page in place; do not append competing
   status snapshots.
2. The implementing agent changes code. A separate Claude review agent owns
   tests and review for each increment. The reviewer must not edit production
   code unless explicitly assigned a separate implementation front.
3. Exchange status and findings through the current project's ai-memory. Never
   copy credentials or private-corpus identifiers into memory.
4. Accept an increment only after formatting, focused tests, full workspace
   tests, clippy with warnings denied, generated-source noise audit, capability
   matrix, and relevant round-trip/oracle checks pass.
5. Commit one coherent, validated increment with a clean worktree. Update the
   pinned backlog to the accepted commit and exact gate counts.

## ai-memory routing

Use `ai-memory` from the repository root without `--project` or `--workspace`,
so it remains scoped to MXRS. Read full relevant decision/rule/gotcha pages,
not only search snippets. Durable decisions and handoffs belong in ai-memory;
the repository contains only product documentation and these operating rules.
