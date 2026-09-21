# Run command contract

`mxrs run [DIR] [--host HOST] [--server-port PORT] [--client-port PORT]
[--environment NAME] [--no-frontend]` ports `mxrb run`'s Supervisor
contract — same defaults (`127.0.0.1`, 9292, 5173), same
`--api-port`/`--port` compatibility aliases (naming the same port twice is
refused), same layered environment loading (`.env`, `.env.NAME`,
`config/environments/NAME.env`), same frontend preflight and termination
behavior — onto a different backend: **mxrs boots its own runtime
in-process** instead of a generated application server.

From the project's built model (`build/<name>.mpr`, produced together with
the `build/web` shell by `cargo mxrs build`):

| Piece | Source |
| --- | --- |
| Store schema | Every named entity, with typed attribute defaults (numeric/boolean defaults parsed, invalid ones refuse the boot) and non-persistable entities marked transient. |
| Security policy | `Security$ProjectSecurity` (user roles → module roles, configured plus `ManageAllRoles` administrators; a present unit with an unknown `SecurityLevel` counts as *enabled*, mxrb's own fail-closed reading) plus per-entity access rules. An XPath-guarded rule is enforced as **deny** until the XPath engine is ported — reported at boot, never silently dropped. |
| Documents | Every named microflow/nanoflow/rule with its allowed module roles. |
| State | SQLite under `DIR/.mxrs/runtime/state.sqlite3`, restored at boot and saved after a graceful shutdown. |
| HTTP | `mxrs-runtime-http`: `/api/health`, `/api/{action|microflow|nanoflow}/{name}`, static `build/web` fallback, loopback origin guard. |

The frontend (unless `--no-frontend`) starts mxrb's way: `npm run dev --
--host H --port P --strictPort` inside `DIR/frontend`, after checking the
package and its `node_modules` exist, with the profile's `VITE_*` variables
plus `MXRS_ENV`/`MXRS_API_PORT`. Interrupting terminates it (TERM, then
reap); the frontend exiting non-zero on its own stops the run with
`frontend process exited with status N`.

## What is not ported yet

Flow *execution*. Invoking a microflow answers `unknown_action` — stated in
the boot banner — until the native flow interpreter (mxrb's
`runtime/native.rb`, executor and access-control evaluation) lands. mxrb's
external-backend presets (`flymetothemoon`/`onrails`) are Ruby-stack
concepts with no mxrs equivalent.
