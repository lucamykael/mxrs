# Run command contract

`mxrs run [DIR] [--host HOST] [--server-port PORT] [--client-port PORT]
[--environment NAME] [--no-frontend] [--allow-destructive-schema]` ports
`mxrb run`'s Supervisor
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
| Security policy | `Security$ProjectSecurity` (user roles → module roles, configured plus `ManageAllRoles` administrators; a present unit with an unknown `SecurityLevel` counts as *enabled*, mxrb's own fail-closed reading) plus per-entity access rules. An XPath-guarded rule narrows to the records its constraint names: the rule applies when the question is about the entity (may this role create at all), and per record when there is one, so a retrieve returns only the rows some applicable rule covers. A constraint outside the evaluated subset denies every record and is reported at boot, never silently dropped. |
| Documents | Every named microflow/nanoflow/rule with its allowed module roles. |
| State | Relational SQLite under `DIR/.mxrs/runtime/state.sqlite3` — the schema-migrated layout of mxrb's `schema_migrator.rb` (GUID-keyed `mxrb_*` tables, unique indexes, `mxrb_schema_*` catalog, byte-compatible with mxrb's runtime databases), restored at boot and saved after a graceful shutdown. Model evolutions migrate in place; destructive ones are refused unless `--allow-destructive-schema` is passed. A pre-existing snapshot-format state file upgrades automatically. |
| HTTP | `mxrs-runtime-http`: `/api/health`, `/api/{action|microflow|nanoflow}/{name}`, static `build/web` fallback, loopback origin guard. |

The frontend (unless `--no-frontend`) starts mxrb's way: `npm run dev --
--host H --port P --strictPort` inside `DIR/frontend`, after checking the
package and its `node_modules` exist, with the profile's `VITE_*` variables
plus `MXRS_ENV`/`MXRS_API_PORT`/`MXRS_API_ORIGIN` (the runtime URL the dev
server proxies `/api` and `/model.json` to). Interrupting terminates it (TERM, then
reap); the frontend exiting non-zero on its own stops the run with
`frontend process exited with status N`.

With `--no-frontend` the runtime serves the project's own frontend build
(`frontend/dist`, with the model's `model.json`) when there is one, and the
embedded shell otherwise — which draws the model's manifest, not the
project's pages, so a project that declares pages and has no build is
warned of it.

## Flow execution

Every named microflow/nanoflow/rule is registered on the native interpreter
(`mxrs-runtime-flows`, the port of mxrb's `runtime/native.rb`):
`POST /api/microflow/<Module.Flow>` runs the model's own logic inside
`Runtime::invoke`'s document authorization + transaction, and answers
`{result, effects, log}` — client-facing activities (messages, page
navigation, downloads, unadapted client actions) surface as `effects`.
Java custom actions and web-service/mapping/document activities require
explicitly registered adapters and fail with named errors otherwise.

## Published REST services

Every operation of every `Rest$PublishedRestService` the model publishes is
served at its own route — the service's path, the resource's name and the
operation's path, `GET /rest/orders/v1/order/{id}` — from the model alone:
a project with no generated HTTP layer serves its services too. A request
signs in as its service asks: anyone, as the project's guest; HTTP Basic
against the model's own accounts, holding one of the service's roles
(`401` with the service's realm, `403` without a role); authentication the
runtime does not offer is refused with `501` rather than served to anyone.
Path and query parameters are read as their types (`400` with why when one
is not), bound to the microflow, and the result answers through the
operation's export mapping — or as the stored object without one — under
the status, and with the content, the flow left on its
`System.HttpResponse`. Headers are not carried yet.

## Scheduled events

Enabled `ScheduledEvents$ScheduledEvent` documents are armed on the ported
scheduler (`mxrs-runtime-scheduler`, mxrb's `runtime/scheduler.rb`): modern
Minute/Hour/Day/Week schedules and the legacy `IntervalType` pair, start
dates, UTC and numeric-offset time zones (IANA names fail explicitly, like
mxrb without tzinfo; `local` is treated as UTC), per-slot lease claims so a
slot never fires twice, and a 1-second poll that executes due microflows on
the native interpreter as system calls. Failures are logged per event; a
scheduler-level failure stops the ticker, mirroring mxrb's run loop.

## What is not ported

mxrb's external-backend presets (`flymetothemoon`/`onrails`) are Ruby-stack
concepts with no mxrs equivalent, and the Mendix-Runtime-toolchain path
(deployment build + real Runtime boot) stays outside this command.
