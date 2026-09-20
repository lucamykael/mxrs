# Demo user scaffold contract

`mxrs demo-user new NAME [--entity Module.Entity] [--role ROLE] [--target DIR]
[--dry-run] [--json]` ports mxrb's `demo-user` recipe to the Cargo-native
layout (`new` is optional, as in native). It writes one declaration under
`src/domain/security/demo_users/`, wires the demo-user aggregator into
`build()` after `security::apply`, and provisions the password:

| Native workspace | Cargo workspace |
| --- | --- |
| `app/security/demo_users/<name>.rb` | `src/domain/security/demo_users/<name>.rs` |
| `security do … evaluate_dir(app/security/demo_users)` loader | `pub mod demo_users;` + `security::demo_users::apply(&mut project);` |
| `.env` (`0600`) with `MXRB_DEMO_USER_<NAME>_PASSWORD` | `.env` (`0600`) with `MXRS_DEMO_USER_<NAME>_PASSWORD` |
| `.env.example` records the key with an empty value | same |

The password value never appears in generated source. The declaration names
the environment variable (`password_env`); `cargo mxrs build` resolves it at
write time. When the variable is absent, a password already stored for a
demo user with the same name is preserved; a brand-new demo user without a
resolvable password fails the build closed
(`MissingDemoUserPassword`).

Writer merge semantics for the stored `DemoUsers` array mirror mxrb's:
entries other than `Security$DemoUserImpl` are opaque and preserved
verbatim, a name match keeps the stored `$ID` and any unmanaged fields, and
an empty declaration list leaves the stored bytes untouched (mxrb rewrites
the array unconditionally; the byte-preserving round-trip contract here
forbids that).

## Validation

Roles must appear in `src/domain/security/mod.rs` (either the
`security.role("Name", …)` scaffold form or the imported
`UserRoleDecl` struct-literal form); the entity must be `System.User` or a
`Module.Entity` whose declaration file exists under
`src/domain/modules/<module>/entities/`. This is the same structural
source-scanning contract mxrb applies to its Ruby projects — not a compiled
model check. The writer additionally rejects demo-user roles that the
project security declaration does not define.

## Status

`Partial`: the generated artifacts are Rust, not mxrb's Ruby, so no
executable command-contract oracle compares the two recipes. Imported demo
users are not exported as typed declarations; they remain losslessly
preserved in the stored model.
