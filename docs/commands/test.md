# Test command contract

`mxrs test FILE.mpr SUITE.json [--plan] [--json]` runs a declarative
functional suite on the MXRS-owned native flow interpreter — the port of
mxrb's `Native::Executor` over `runtime/native.rb`. `--plan` keeps the
validation-only mode (every target, hook, argument and count entity checked
against the MPR without executing anything).

Execution follows the oracle's contract exactly:

- One interpreter and one store are shared across the whole suite; each
  root call is its own unit of work (rollback on error, uncommitted work
  discarded on success).
- Per test: `before` hook → the `call` target with `pass` arguments (each
  argument string is a Mendix expression evaluated standalone) →
  expectations → `after` hook.
- Expectations: `expect.return` compares against the evaluated expected
  expression; `expect.count` entries count stored objects, optionally
  narrowed by a native XPath constraint.
- Any error fails that test with its message; the suite continues.
- Output is mxrb's transcript shape (`[MXRS_TEST] PASS name` /
  `[MXRS_TEST] FAIL name` / `[MXRS_TEST] DONE`), failing tests are listed
  with their messages, and the exit status is nonzero unless every test
  passed. `--json` serializes the full report.

## What differs from mxrb

mxrb's alternative executor — building a deployment with the Mendix
toolchain and driving the real Mendix Runtime — is not ported; the native
interpreter is the only execution engine. Unsupported activities and
unregistered Java/client/web-service adapters fail tests with named
errors instead of pretending to run.
